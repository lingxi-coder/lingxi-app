//! Cross-process live-session registry + background-job store readers — the
//! data source for `lingxi-cli agents --json` and the interactive agent view.
//!
//! (M7 cc2.1.198) Ports the real binary's `printAgentsJson` pipeline
//! (2.1.198 @223853400, module `mXc`/`pGf`):
//!
//! * **Live sessions** — every running claude process registers itself as
//!   `<config-home>/sessions/<pid>.json` (observed shape: `{pid, sessionId,
//!   cwd, startedAt, procStart, version, peerProtocol, kind, entrypoint,
//!   name, nameSource, status, updatedAt, statusUpdatedAt}`; bg workers also
//!   carry `jobId`). `bKe()` reads the dir and drops entries whose pid is no
//!   longer alive. lingxi mirrors the same layout under
//!   `$LINGXI_CONFIG_DIR`/`~/.lingxi/sessions/`.
//! * **Background jobs** — `<config-home>/jobs/<short>/state.json` (observed
//!   shape: `{state, tempo, name, sessionId, cwd, originCwd, createdAt,
//!   intent, displayIntent, template, respawnFlags, inFlight, …}`). lingxi
//!   does not yet WRITE jobs (`--bg` dispatch is M8); the reader is landed
//!   now so `agents --json` picks them up the moment the writer exists.
//! * **Merge** — jobs first (live worker matched by `jobId`), then live
//!   sessions not consumed by a job; sorted by `startedAt` ascending; output
//!   keys in the binary's exact insertion order (`preserve_order` keeps
//!   `serde_json::Map` faithful): `{pid?, id, cwd, kind, startedAt,
//!   sessionId, name?, status?, waitingFor?, state}` for job rows and
//!   `{pid, cwd, kind, startedAt, sessionId?, name?, status?, waitingFor?}`
//!   for live-only rows.
//!
//! State model (binary `mGf`/`$re`/`FI`/`Xg`/`lDe`, @209972919): see
//! [`merged_state`]. The 2.1.196 "no Done↔Needs-input flip" fix is exactly
//! the terminal-outcome precedence in `mGf` — once a job is terminal it
//! reports `done`/`failed`/`stopped` even if its tempo is still `blocked`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `sessions/` under the config home — one `<pid>.json` per live process.
#[must_use]
pub fn sessions_dir(config_home: &Path) -> PathBuf {
    config_home.join("sessions")
}

/// `jobs/` under the config home — one `<short>/state.json` per background job.
#[must_use]
pub fn jobs_dir(config_home: &Path) -> PathBuf {
    config_home.join("jobs")
}

/// One live-process registration (`sessions/<pid>.json`). Field order matches
/// the observed on-disk order of the real 2.1.198 binary byte-for-byte (the
/// registration file is itself a parity surface).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSessionRecord {
    /// Registering process id.
    pub pid: i32,
    /// Session UUID (absent for processes that never minted a session).
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Process working directory.
    pub cwd: String,
    /// Epoch milliseconds the session started.
    #[serde(rename = "startedAt")]
    pub started_at: i64,
    /// `ps -o lstart` string of the registering process — pid-reuse guard.
    #[serde(rename = "procStart", skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<String>,
    /// CLI version that wrote the record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Peer protocol version (binary writes `1`).
    #[serde(rename = "peerProtocol", skip_serializing_if = "Option::is_none")]
    pub peer_protocol: Option<u32>,
    /// `"interactive"` or `"bg"` (binary `CLAUDE_CODE_SESSION_KIND`).
    pub kind: String,
    /// Job short id for bg workers (`jobs/<jobId>/`).
    #[serde(rename = "jobId", skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Entry point that started the process (binary writes `"cli"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Display name (binary derives one from the project dir).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `"derived"` / `"user"` — where `name` came from.
    #[serde(rename = "nameSource", skip_serializing_if = "Option::is_none")]
    pub name_source: Option<String>,
    /// Live status: `"idle"` / `"busy"` / `"waiting"` / `"shell"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// What a `"waiting"` session is waiting for.
    #[serde(rename = "waitingFor", skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    /// Epoch ms of the last record refresh.
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// Epoch ms of the last `status` change.
    #[serde(rename = "statusUpdatedAt", skip_serializing_if = "Option::is_none")]
    pub status_updated_at: Option<i64>,
    /// Epoch ms the current advertised name was claimed.
    #[serde(rename = "nameSince", skip_serializing_if = "Option::is_none")]
    pub name_since: Option<i64>,
    /// Previous advertised names (2.1.232 uniqueness).
    #[serde(rename = "formerNames", skip_serializing_if = "Option::is_none")]
    pub former_names: Option<Vec<String>>,
    /// Unix-domain inbox path (2.1.232 `messagingSocketPath`).
    #[serde(
        rename = "messagingSocketPath",
        skip_serializing_if = "Option::is_none"
    )]
    pub messaging_socket_path: Option<String>,
    /// Attested permission class (`bypass` / `prompting`) for inbound `g6f`.
    #[serde(rename = "permissionClass", skip_serializing_if = "Option::is_none")]
    pub permission_class: Option<String>,
}

/// In-flight task counters inside a job's `state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobInFlight {
    /// Running task count.
    #[serde(default)]
    pub tasks: u32,
    /// Queued task count.
    #[serde(default)]
    pub queued: u32,
    /// Kinds of in-flight tasks (e.g. `"session_cron"`).
    #[serde(default)]
    pub kinds: Vec<String>,
}

/// A background job's persisted state (`jobs/<short>/state.json`). Only the
/// fields `printAgentsJson` consumes are modeled; unknown fields are ignored
/// so richer binary-written stores still parse.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct JobState {
    /// Job lifecycle state: `working`/`blocked`/`done`/`failed`/`stopped`/….
    #[serde(default)]
    pub state: String,
    /// Job tempo: `active`/`blocked`/`idle`.
    #[serde(default)]
    pub tempo: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Original dispatch intent.
    #[serde(default)]
    pub intent: Option<String>,
    /// Overridden display intent.
    #[serde(rename = "displayIntent", default)]
    pub display_intent: Option<String>,
    /// First prompt of the job session.
    #[serde(rename = "initialPrompt", default)]
    pub initial_prompt: Option<String>,
    /// Session UUID of the worker.
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    /// Worker cwd (often a managed worktree).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Where the job was dispatched from (pre-worktree cwd).
    #[serde(rename = "originCwd", default)]
    pub origin_cwd: Option<String>,
    /// ISO-8601 creation timestamp.
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<String>,
    /// Attached routine name, if the job runs one.
    #[serde(default)]
    pub routine: Option<serde_json::Value>,
    /// In-flight task counters.
    #[serde(rename = "inFlight", default)]
    pub in_flight: Option<JobInFlight>,
    /// Job detail line (may reference a PR — surfaced in the agent view).
    #[serde(default)]
    pub detail: Option<String>,
    /// What a `blocked` job needs from the user (the question text, or the
    /// binary's fresh-session sentinel `"send a prompt to start"`). Feeds the
    /// `agent_needs_input` Notification message (binary `$1f` @222691698).
    #[serde(default)]
    pub needs: Option<String>,
    /// Dispatch template name (`"bg"`, `"exec"`, an agent name, …). Binary
    /// `ISe` (@209973132) excludes `exec` one-shots from notifications.
    #[serde(default)]
    pub template: Option<String>,
    /// Job backend (`"daemon"` for locally daemon-backed workers). The
    /// notification watcher only observes daemon-backed jobs (binary FleetView
    /// filter `lt.state.backend==="daemon"` @222750113).
    #[serde(default)]
    pub backend: Option<String>,
    /// Flags a respawn re-applies. Part of the `ISe` exec-one-shot test
    /// (`template==="exec" && respawnFlags.length===0`).
    #[serde(rename = "respawnFlags", default)]
    pub respawn_flags: Vec<String>,
    /// OS pid of the live worker process executing this job. Recorded by the
    /// daemon supervisor when it spawns the detached `__bg-run` worker and
    /// cleared when the job reaches a terminal state. The supervisor's
    /// cross-restart double-spawn guard reads it: a job whose recorded
    /// `workerPid` is still alive is not re-spawned. Appended AFTER the pinned
    /// observed keys (the reader ignores unknown fields, so this is additive).
    #[serde(rename = "workerPid", default)]
    pub worker_pid: Option<i32>,
}

/// `dXc` — sanitize a display name: strip C0/C1 control chars
/// (`[\x00-\x08\x0E-\x1F\x7F-\x9F]`), collapse runs of whitespace to one
/// space, trim. Returns `None` when nothing survives (the binary omits the
/// `name` key via `...f&&{name:f}`).
#[must_use]
pub fn sanitize_name(raw: &str) -> Option<String> {
    let stripped: String = raw
        .chars()
        .filter(|c| {
            !matches!(*c,
                '\u{0}'..='\u{8}' | '\u{e}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
        })
        .collect();
    let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

/// `pXc` — normalize a live status for output: `idle` and `waiting` pass
/// through, anything else (busy/shell/…) reports as `busy`.
#[must_use]
pub fn normalize_status(status: &str) -> &'static str {
    match status {
        "idle" => "idle",
        "waiting" => "waiting",
        _ => "busy",
    }
}

/// `$re` — map a terminal job state to its outcome; `None` for live states.
#[must_use]
pub fn terminal_outcome(state: &str) -> Option<&'static str> {
    match state {
        "done" => Some("success"),
        "failed" => Some("failure"),
        "stopped" => Some("stopped"),
        _ => None,
    }
}

/// `Xg` — a job is terminal when its state has a terminal outcome AND its
/// tempo is no longer `active`.
#[must_use]
pub fn job_is_terminal(job: &JobState) -> bool {
    terminal_outcome(&job.state).is_some() && job.tempo.as_deref() != Some("active")
}

/// `lDe` — "loopish" jobs (routine attached, `session_cron` in flight, or a
/// `/loop` intent) stay listed even after a `success` outcome.
#[must_use]
pub fn job_is_loopish(job: &JobState) -> bool {
    let loop_intent = |s: &Option<String>| {
        s.as_deref()
            .is_some_and(|v| v.trim().to_lowercase().starts_with("/loop"))
    };
    job.routine.is_some()
        || job
            .in_flight
            .as_ref()
            .is_some_and(|f| f.kinds.iter().any(|k| k == "session_cron"))
        || loop_intent(&job.intent)
        || loop_intent(&job.initial_prompt)
}

/// `mGf` — merge a job's persisted state with its live worker status into the
/// reported `state`: `working` / `blocked` / `done` / `failed` / `stopped`.
///
/// Terminal outcomes take precedence over a stale `blocked` tempo — the
/// 2.1.196 "sessions no longer flip between Done and Needs-input" fix: a
/// `done` job with `tempo: "blocked"` still reports `done`.
#[must_use]
pub fn merged_state(job: &JobState, live_status: Option<&str>) -> &'static str {
    if live_status == Some("busy") {
        return "working";
    }
    let outcome = terminal_outcome(&job.state);
    if job_is_terminal(job) && !(outcome == Some("success") && job_is_loopish(job)) {
        return match outcome {
            Some("success") => "done",
            Some("failure") => "failed",
            _ => "stopped",
        };
    }
    if job.tempo.as_deref() == Some("blocked") || live_status == Some("waiting") {
        return "blocked";
    }
    "working"
}

/// `HSe` — the cwd a job is grouped under: `originCwd`, else the worktree
/// prefix stripped from `cwd` (`^(.+?)/<DOT_DIR>/worktrees/…` → the project
/// root), else `cwd` as-is.
#[must_use]
pub fn job_origin_cwd(job: &JobState) -> String {
    if let Some(origin) = job.origin_cwd.as_deref() {
        if !origin.is_empty() {
            return origin.to_string();
        }
    }
    let cwd = job.cwd.as_deref().unwrap_or("");
    for sep in ['/', '\\'] {
        let marker = format!("{sep}{}{sep}worktrees{sep}", branding::DOT_DIR);
        if let Some(idx) = cwd.find(&marker) {
            return cwd[..idx].to_string();
        }
    }
    cwd.to_string()
}

/// The `--cwd <path>` filter (`r(d)` in `pGf`): keep entries whose cwd is the
/// filter root or beneath it — `path.relative(root, d)` must not start with
/// `..` and must not be absolute. `None` filter keeps everything.
#[must_use]
pub fn cwd_matches(filter: Option<&Path>, cwd: &str) -> bool {
    let Some(root) = filter else { return true };
    Path::new(cwd).strip_prefix(root).is_ok()
}

/// Parse an ISO-8601 `createdAt` into epoch milliseconds (`Date.parse`).
/// Unparseable input degrades to `0` (sorts first) rather than dropping the
/// row.
#[must_use]
pub fn parse_created_at_ms(created_at: Option<&str>) -> i64 {
    created_at
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or(0, |dt| dt.timestamp_millis())
}

/// Build the `agents --json` array — the faithful `printAgentsJson` merge.
///
/// * `live` — live-session records (already liveness-filtered).
/// * `jobs` — `(short_id, state)` pairs from the jobs store.
/// * `cwd_filter` — resolved `--cwd` root.
/// * `all` — with `--all`, completed (terminal, workerless) jobs are kept;
///   without it only `working`/`blocked` or live-backed rows survive.
#[must_use]
// One function = one faithful `pGf` port; splitting the two merge loops apart
// would scatter the binary's field-order/filter contract across helpers.
#[allow(clippy::too_many_lines)]
pub fn build_agents_json(
    live: &[LiveSessionRecord],
    jobs: &[(String, JobState)],
    cwd_filter: Option<&Path>,
    all: bool,
) -> Vec<serde_json::Value> {
    use serde_json::{json, Map, Value};

    // Live bg workers keyed by jobId (`a` in pGf).
    let mut worker_by_job: std::collections::HashMap<&str, &LiveSessionRecord> =
        std::collections::HashMap::new();
    for rec in live {
        if rec.kind == "bg" {
            if let Some(job_id) = rec.job_id.as_deref() {
                worker_by_job.insert(job_id, rec);
            }
        }
    }

    let mut rows: Vec<Value> = Vec::new();
    let mut consumed_pids: std::collections::HashSet<i32> = std::collections::HashSet::new();

    // Job rows first.
    for (short, job) in jobs {
        let worker = worker_by_job.get(short.as_str()).copied();
        if let Some(w) = worker {
            consumed_pids.insert(w.pid);
        }
        if !cwd_matches(cwd_filter, &job_origin_cwd(job)) {
            continue;
        }
        let state = merged_state(job, worker.and_then(|w| w.status.as_deref()));
        if !all && worker.is_none() && state != "working" && state != "blocked" {
            continue;
        }
        let name = worker
            .and_then(|w| w.name.as_deref())
            .or(job.name.as_deref())
            .or(job.display_intent.as_deref())
            .or(job.intent.as_deref())
            .and_then(sanitize_name);

        // Exact binary key order: pid?, id, cwd, kind, startedAt, sessionId,
        // name?, status?, waitingFor?, state.
        let mut m = Map::new();
        if let Some(w) = worker {
            m.insert("pid".into(), json!(w.pid));
        }
        m.insert("id".into(), json!(short));
        let cwd = worker
            .map(|w| w.cwd.clone())
            .or_else(|| job.cwd.clone())
            .unwrap_or_default();
        m.insert("cwd".into(), json!(cwd));
        m.insert("kind".into(), json!("background"));
        let started_at = worker.map_or_else(
            || parse_created_at_ms(job.created_at.as_deref()),
            |w| w.started_at,
        );
        m.insert("startedAt".into(), json!(started_at));
        let session_id = worker
            .and_then(|w| w.session_id.clone())
            .or_else(|| job.session_id.clone())
            .unwrap_or_default();
        m.insert("sessionId".into(), json!(session_id));
        if let Some(n) = name {
            m.insert("name".into(), json!(n));
        }
        if let Some(status) = worker.and_then(|w| w.status.as_deref()) {
            m.insert("status".into(), json!(normalize_status(status)));
            if status == "waiting" {
                if let Some(wf) = worker.and_then(|w| w.waiting_for.as_deref()) {
                    m.insert("waitingFor".into(), json!(wf));
                }
            }
        }
        m.insert("state".into(), json!(state));
        rows.push(Value::Object(m));
    }

    // Live-only rows (interactive sessions + orphan bg workers).
    for rec in live {
        if rec.kind != "interactive" && rec.kind != "bg" {
            continue;
        }
        if consumed_pids.contains(&rec.pid) {
            continue;
        }
        if rec.kind == "bg" && rec.job_id.is_some() {
            continue;
        }
        if !cwd_matches(cwd_filter, &rec.cwd) {
            continue;
        }
        let mut m = Map::new();
        m.insert("pid".into(), json!(rec.pid));
        m.insert("cwd".into(), json!(rec.cwd));
        m.insert(
            "kind".into(),
            json!(if rec.kind == "bg" {
                "background"
            } else {
                "interactive"
            }),
        );
        m.insert("startedAt".into(), json!(rec.started_at));
        if let Some(sid) = &rec.session_id {
            m.insert("sessionId".into(), json!(sid));
        }
        if let Some(n) = rec.name.as_deref().and_then(sanitize_name) {
            m.insert("name".into(), json!(n));
        }
        if let Some(status) = rec.status.as_deref() {
            m.insert("status".into(), json!(normalize_status(status)));
            if status == "waiting" {
                if let Some(wf) = rec.waiting_for.as_deref() {
                    m.insert("waitingFor".into(), json!(wf));
                }
            }
        }
        rows.push(Value::Object(m));
    }

    // `c.sort((d,p)=>d.startedAt-p.startedAt)` — ascending, stable.
    rows.sort_by_key(|v| v.get("startedAt").and_then(serde_json::Value::as_i64));
    rows
}

/// Whether `pid` is a live process (`kill(pid, 0)` — signal 0 probes without
/// sending). `EPERM` still means alive (a process we can't signal exists).
#[cfg(unix)]
#[must_use]
pub fn process_alive(pid: i32) -> bool {
    // EPERM still means a live process (one we may not signal).
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Ok(()) | Err(nix::errno::Errno::EPERM)
    )
}

#[cfg(not(unix))]
#[must_use]
pub fn process_alive(pid: i32) -> bool {
    crate::daemon_roster::ProcProbe::is_alive(&crate::daemon_roster::SystemProbe, pid)
}

/// Read all live-session records under `dir`, dropping records whose pid is
/// dead (and best-effort unlinking those stale files, mirroring the binary's
/// reaper). Unreadable/unparseable files are skipped.
#[must_use]
pub fn read_live_sessions(dir: &Path) -> Vec<LiveSessionRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rec) = serde_json::from_str::<LiveSessionRecord>(&bytes) else {
            continue;
        };
        if !process_alive(rec.pid) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        out.push(rec);
    }
    // Deterministic order for the downstream stable sort (read_dir order is
    // platform-dependent).
    out.sort_by_key(|r| r.pid);
    out
}

/// Read all background jobs under `dir` (`<short>/state.json`), sorted by
/// short id for determinism. Missing/unparseable state files are skipped.
#[must_use]
pub fn read_jobs(dir: &Path) -> Vec<(String, JobState)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(short) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(bytes) = std::fs::read_to_string(path.join("state.json")) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<JobState>(&bytes) else {
            continue;
        };
        out.push((short.to_string(), state));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------------------
// Job WRITER (M8 `--bg` dispatch) — the durable `jobs/<short>/state.json`.
// ---------------------------------------------------------------------------

/// Serializer twin of [`JobState`] (which is `Deserialize`-only). Fields carry
/// the SAME `#[serde(rename=…)]`/`skip_serializing_if` attrs as the reader and
/// are declared in the pinned observed key order so the written `state.json` is
/// key-order-faithful: `state, tempo, name, sessionId, cwd, originCwd,
/// createdAt, intent, displayIntent, template, respawnFlags, inFlight` followed
/// by the observed extras `backend, initialPrompt`.
///
/// Borrows its string fields so [`dispatch_background`](crate::background_dispatch)
/// can build one straight off a freshly-minted job without cloning.
#[derive(Debug, Clone, Serialize)]
pub struct JobStateWrite<'a> {
    /// Job lifecycle state — `"working"` for a fresh `--bg` prompt so
    /// [`build_agents_json`] keeps the workerless row without `--all`.
    pub state: &'a str,
    /// Job tempo — `"active"` for a fresh job (MUST NOT be `"blocked"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo: Option<&'a str>,
    /// Display name (absent — the label falls back to `intent`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// Session UUID of the (future) worker.
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<&'a str>,
    /// Worker cwd.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<&'a str>,
    /// Dispatch-origin cwd.
    #[serde(rename = "originCwd", skip_serializing_if = "Option::is_none")]
    pub origin_cwd: Option<&'a str>,
    /// RFC3339-millis-Z creation timestamp (a no-offset stamp makes
    /// `parse_created_at_ms` degrade to `0`).
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<&'a str>,
    /// Original dispatch intent (first prompt line).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<&'a str>,
    /// Overridden display intent (absent).
    #[serde(rename = "displayIntent", skip_serializing_if = "Option::is_none")]
    pub display_intent: Option<&'a str>,
    /// Dispatch template (`"bg"` for a `--bg` prompt job — NOT `"exec"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<&'a str>,
    /// Flags a respawn re-applies (`[]` for a fresh job).
    #[serde(rename = "respawnFlags")]
    pub respawn_flags: &'a [String],
    /// In-flight task counters (absent for a fresh job).
    #[serde(rename = "inFlight", skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<&'a JobInFlight>,
    /// Job backend (`"daemon"` so `agents_notify::notify_rows` observes it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<&'a str>,
    /// First prompt of the job session.
    #[serde(rename = "initialPrompt", skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<&'a str>,
    /// Failure/status detail line surfaced in the agent view (e.g. the
    /// `spawn_cwd_gone` "working directory no longer exists…" message the daemon
    /// stamps when it fails a job whose cwd vanished). Omitted (`None`) for a
    /// fresh `--bg` job — `skip_serializing_if` keeps a fresh job's `state.json`
    /// byte-unchanged, and the reader tolerates the additive key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<&'a str>,
    /// OS pid of the live worker executing this job (recorded by the daemon
    /// supervisor on spawn; cleared — omitted — when the job reaches a terminal
    /// state). Serialized LAST so the pinned observed key order (through
    /// `initialPrompt`) is preserved; `skip_serializing_if` keeps a `None` off
    /// disk entirely, so a fresh `--bg` job's `state.json` is byte-unchanged.
    #[serde(rename = "workerPid", skip_serializing_if = "Option::is_none")]
    pub worker_pid: Option<i32>,
}

/// Write `jobs/<short>/state.json` atomically (create the dir, write a
/// `state.json.tmp.<pid>` sibling, then rename into place — [`read_jobs`]
/// silently drops a torn `state.json`, so the rename is what makes the row
/// visible). Compact JSON: the reader ([`serde_json::from_str`]) tolerates
/// either form and `state.json` files are conventionally unindented.
pub fn write_job_state(
    config_home: &Path,
    short: &str,
    job: &JobStateWrite,
) -> std::io::Result<()> {
    let dir = jobs_dir(config_home).join(short);
    if let Ok(metadata) = std::fs::symlink_metadata(&dir) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "background job path is not a real directory",
            ));
        }
    }
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let body = serde_json::to_string(job)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = dir.join(format!(
        "state.json.tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, body.as_bytes())?;
    let target = dir.join("state.json");
    match std::fs::rename(&tmp, &target) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Read a SINGLE background job's `jobs/<short>/state.json` into a [`JobState`].
/// `None` when the dir/file is missing or the JSON is unparseable (the same
/// tolerance as [`read_jobs`]). The `--bg` worker uses this only for public
/// lifecycle identity; private prompt/runtime context comes from `launch.json`.
#[must_use]
pub fn read_job(config_home: &Path, short: &str) -> Option<JobState> {
    let path = jobs_dir(config_home).join(short).join("state.json");
    let bytes = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<JobState>(&bytes).ok()
}

/// Read-modify-write `jobs/<short>/state.json` to a new `state`, preserving the
/// pinned key order (it rebuilds a [`JobStateWrite`] from the existing
/// [`JobState`] and only overrides `state`/`tempo`/`workerPid`). Used by both:
///
/// * the daemon **supervisor**, to record the spawned worker's `workerPid`
///   while the job stays `state:"working"` (its cross-restart double-spawn
///   guard), and
/// * the `__bg-run` **worker**, to stamp the reader-recognized terminal state
///   (`"done"` on success, `"failed"` on error) and clear `workerPid`.
///
/// Terminal states ([`terminal_outcome`] `Some`) force `tempo:"idle"` so
/// [`job_is_terminal`] reports the job terminal (which requires `tempo !=
/// "active"`); non-terminal updates keep the existing tempo. `worker_pid`
/// overwrites the stored value verbatim (`None` clears it).
///
/// Errors if the job does not exist (there is nothing to modify).
pub fn update_job_state(
    config_home: &Path,
    short: &str,
    new_state: &str,
    worker_pid: Option<i32>,
) -> std::io::Result<()> {
    update_job_state_inner(config_home, short, new_state, worker_pid, None)
}

/// [`update_job_state`] that also stamps a `detail` line on the job. Used by the
/// daemon supervisor to record WHY a job failed — e.g. the byte-faithful
/// `spawn_cwd_gone` detail `working directory no longer exists or is not
/// accessible: <cwd>` (CC's `settleCwdGone`) — so the agent view can surface it.
pub fn update_job_state_with_detail(
    config_home: &Path,
    short: &str,
    new_state: &str,
    worker_pid: Option<i32>,
    detail: &str,
) -> std::io::Result<()> {
    update_job_state_inner(config_home, short, new_state, worker_pid, Some(detail))
}

/// Read-modify-write core shared by [`update_job_state`] /
/// [`update_job_state_with_detail`]. `detail: None` preserves the fixed
/// (detail-less) write shape existing callers depend on; `Some` stamps the
/// detail line.
fn update_job_state_inner(
    config_home: &Path,
    short: &str,
    new_state: &str,
    worker_pid: Option<i32>,
    detail: Option<&str>,
) -> std::io::Result<()> {
    let existing = read_job(config_home, short).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("job {short} has no state.json to update"),
        )
    })?;
    // A terminal outcome must leave tempo != "active" (else `job_is_terminal`
    // stays false and the row keeps rendering as "working").
    let tempo: Option<String> = if terminal_outcome(new_state).is_some() {
        Some("idle".to_string())
    } else {
        existing.tempo.clone()
    };
    let job = JobStateWrite {
        state: new_state,
        tempo: tempo.as_deref(),
        name: existing.name.as_deref(),
        session_id: existing.session_id.as_deref(),
        cwd: existing.cwd.as_deref(),
        origin_cwd: existing.origin_cwd.as_deref(),
        created_at: existing.created_at.as_deref(),
        intent: existing.intent.as_deref(),
        display_intent: existing.display_intent.as_deref(),
        template: existing.template.as_deref(),
        respawn_flags: &existing.respawn_flags,
        in_flight: existing.in_flight.as_ref(),
        backend: existing.backend.as_deref(),
        initial_prompt: existing.initial_prompt.as_deref(),
        detail,
        worker_pid,
    };
    write_job_state(config_home, short, &job)
}

/// Mint a fresh 8-char lowercase-hex short id whose `jobs/<short>/` dir does not
/// already exist (the pinned fixture format, e.g. `bc7c6b33`). Derives the hex
/// from a v4 UUID.
#[must_use]
pub fn mint_short_id(config_home: &Path) -> String {
    mint_short_id_with(config_home, &mut || {
        uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
    })
}

/// [`mint_short_id`] with an injected id generator so the dir-collision retry is
/// testable without depending on UUID entropy.
fn mint_short_id_with(config_home: &Path, generate: &mut dyn FnMut() -> String) -> String {
    let dir = jobs_dir(config_home);
    loop {
        let short = generate();
        if !dir.join(&short).exists() {
            return short;
        }
    }
}

/// Register the current process in the live-session registry and remove the
/// record on drop. Best-effort on both sides: registration failure is
/// swallowed (a session must never die for lack of a registry write), and a
/// failed unlink leaves a stale record the next reader reaps via the
/// liveness probe.
///
/// (M8 cc2.1.198) Live status refreshes: [`Self::update_status`] rewrites the
/// record when the session's status changes (idle ↔ busy ↔ waiting), porting
/// the binary's `mvn` (@222989611 caller): the patch always bumps `updatedAt`
/// and — because the status key is always present in the patch — also
/// `statusUpdatedAt`. The record + path live behind a `Mutex` so the async
/// forwarders in `mode.rs` can share one registration via `Arc`.
pub struct SessionRegistration {
    inner: std::sync::Mutex<RegistrationInner>,
}

struct RegistrationInner {
    path: Option<PathBuf>,
    record: Option<LiveSessionRecord>,
}

impl SessionRegistration {
    /// Write `<config-home>/sessions/<pid>.json` for this process.
    #[must_use]
    pub fn register(config_home: &Path, session_id: Option<&str>, name: Option<&str>) -> Self {
        Self::register_kind(config_home, session_id, name, "interactive", None)
    }

    /// Register this process as a **background worker** — `kind:"bg"` +
    /// `jobId:<short>` (vs [`register`](Self::register)'s hardcoded
    /// `kind:"interactive"`/`jobId:None`). This is the seam the future
    /// pty-backed worker process calls so `build_agents_json` can match the live
    /// worker to its `jobs/<short>/state.json` row by `jobId`.
    ///
    /// NOTE (daemon coherent minimum): the transient `--bg` CLI process exits
    /// immediately, so registering `sessions/<cli_pid>.json` here would be pruned
    /// on the next dead-pid sweep — visibility instead rides on the workerless
    /// `state:"working"` job row. This method exists as the API the real worker
    /// will use; the `--bg` dispatcher does not call it.
    #[must_use]
    pub fn register_bg(
        config_home: &Path,
        session_id: Option<&str>,
        name: Option<&str>,
        job_id: &str,
    ) -> Self {
        Self::register_kind(config_home, session_id, name, "bg", Some(job_id))
    }

    /// Shared record-write for [`register`](Self::register) /
    /// [`register_bg`](Self::register_bg): the only differences are `kind` and
    /// `jobId`.
    #[must_use]
    fn register_kind(
        config_home: &Path,
        session_id: Option<&str>,
        name: Option<&str>,
        kind: &str,
        job_id: Option<&str>,
    ) -> Self {
        let pid = std::process::id();
        let pid = i32::try_from(pid).unwrap_or(i32::MAX);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let record = LiveSessionRecord {
            pid,
            session_id: session_id.map(str::to_string),
            cwd,
            started_at: now_ms,
            // Uniqueness (`c1_`) only treats records with `procStart` as holders.
            proc_start: Some(now_ms.to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            peer_protocol: Some(1),
            kind: kind.to_string(),
            job_id: job_id.map(str::to_string),
            entrypoint: Some("cli".to_string()),
            name: name.map(str::to_string),
            name_source: name.map(|_| "derived".to_string()),
            status: Some("idle".to_string()),
            waiting_for: None,
            updated_at: Some(now_ms),
            status_updated_at: Some(now_ms),
            name_since: Some(now_ms),
            former_names: None,
            messaging_socket_path: None,
            permission_class: None,
        };
        let dir = sessions_dir(config_home);
        let path = dir.join(format!("{pid}.json"));
        let ok = std::fs::create_dir_all(&dir).is_ok()
            && serde_json::to_string(&record)
                .ok()
                .is_some_and(|s| std::fs::write(&path, s).is_ok());
        Self {
            inner: std::sync::Mutex::new(RegistrationInner {
                path: ok.then_some(path),
                record: ok.then_some(record),
            }),
        }
    }

    /// Rewrite this session's registry record with a new live `status` (+
    /// `waitingFor`) — the binary's `mvn` (`{...patch, updatedAt: now,
    /// statusUpdatedAt: now}`; the caller's `useEffect` deps mean it only
    /// fires when the `(status, waitingFor)` pair actually changed, mirrored
    /// by the no-op guard here). Best-effort: a write failure leaves the old
    /// record; a registration that never landed is a no-op.
    ///
    /// `status` is the raw live status (`"idle"`/`"busy"`/`"waiting"`);
    /// `waiting_for` is the binary's waiting reason taxonomy
    /// (`"permission prompt"` / `"worker request"` / `"sandbox request"` /
    /// `"dialog open"` / `"input needed"`), `None` unless `status ==
    /// "waiting"` (binary: `Pb = Za!=="waiting" ? void 0 : …`).
    pub fn update_status(&self, status: &str, waiting_for: Option<&str>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let RegistrationInner {
            path: Some(path),
            record: Some(record),
        } = &mut *inner
        else {
            return;
        };
        if record.status.as_deref() == Some(status) && record.waiting_for.as_deref() == waiting_for
        {
            return;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        record.status = Some(status.to_string());
        record.waiting_for = waiting_for.map(str::to_string);
        record.updated_at = Some(now_ms);
        record.status_updated_at = Some(now_ms);
        if let Ok(s) = serde_json::to_string(record) {
            let _ = std::fs::write(path, s);
        }
    }

    /// Update the advertised name after a uniqueness claim or `/rename`.
    pub fn set_name(&self, name: &str, source: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let RegistrationInner {
            path: Some(path),
            record: Some(record),
        } = &mut *inner
        else {
            return;
        };
        let now_ms = chrono::Utc::now().timestamp_millis();
        if record.name.as_deref() != Some(name) {
            if let Some(prev) = record.name.clone() {
                record.former_names.get_or_insert_with(Vec::new).push(prev);
            }
        }
        record.name = Some(name.to_string());
        record.name_source = Some(source.to_string());
        record.name_since = Some(now_ms);
        record.updated_at = Some(now_ms);
        if let Ok(s) = serde_json::to_string(record) {
            let _ = std::fs::write(path, s);
        }
    }

    /// Record the UDS inbox path (2.1.232 `messagingSocketPath`).
    pub fn set_messaging_socket(&self, sock: &std::path::Path) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let RegistrationInner {
            path: Some(path),
            record: Some(record),
        } = &mut *inner
        else {
            return;
        };
        record.messaging_socket_path = Some(sock.display().to_string());
        record.updated_at = Some(chrono::Utc::now().timestamp_millis());
        if let Ok(s) = serde_json::to_string(record) {
            let _ = std::fs::write(path, s);
        }
    }

    /// Remove the registry record NOW (idempotent; `Drop` calls the same).
    /// The `mode.rs` mount calls this explicitly when the TUI returns, so the
    /// unlink never waits on a status-forwarder task that still holds an
    /// `Arc` clone. Subsequent [`Self::update_status`] calls are no-ops.
    pub fn deregister(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(path) = inner.path.take() {
                let _ = std::fs::remove_file(path);
            }
            inner.record = None;
        }
    }
}

impl Drop for SessionRegistration {
    fn drop(&mut self) {
        self.deregister();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(state: &str, tempo: Option<&str>) -> JobState {
        JobState {
            state: state.to_string(),
            tempo: tempo.map(str::to_string),
            ..JobState::default()
        }
    }

    #[test]
    fn sanitize_name_strips_controls_and_collapses_whitespace() {
        // dXc: control-strip → whitespace-collapse → trim.
        assert_eq!(sanitize_name(" a\u{7}b \t\n c "), Some("ab c".to_string()));
        assert_eq!(sanitize_name("\u{1}\u{2} \t "), None);
    }

    #[test]
    fn normalize_status_maps_everything_else_to_busy() {
        assert_eq!(normalize_status("idle"), "idle");
        assert_eq!(normalize_status("waiting"), "waiting");
        assert_eq!(normalize_status("busy"), "busy");
        assert_eq!(normalize_status("shell"), "busy");
    }

    #[test]
    fn merged_state_busy_worker_is_working() {
        // mGf line 1: a busy live worker overrides everything.
        assert_eq!(merged_state(&job("done", None), Some("busy")), "working");
    }

    #[test]
    fn merged_state_terminal_beats_blocked_tempo_no_done_needs_input_flip() {
        // The 2.1.196 stable-status fix: a terminal job with a stale
        // `blocked` tempo reports its terminal outcome, NOT "blocked" — so
        // the agent view can't flip Done ↔ Needs-input.
        assert_eq!(merged_state(&job("done", Some("blocked")), None), "done");
        assert_eq!(
            merged_state(&job("failed", Some("blocked")), None),
            "failed"
        );
        assert_eq!(
            merged_state(&job("stopped", Some("blocked")), None),
            "stopped"
        );
    }

    #[test]
    fn merged_state_active_tempo_keeps_terminal_state_live() {
        // Xg requires tempo !== "active": a done-state job whose tempo is
        // still active is not terminal yet.
        assert_eq!(merged_state(&job("done", Some("active")), None), "working");
    }

    #[test]
    fn merged_state_blocked_tempo_and_waiting_worker() {
        assert_eq!(
            merged_state(&job("working", Some("blocked")), None),
            "blocked"
        );
        assert_eq!(
            merged_state(&job("working", None), Some("waiting")),
            "blocked"
        );
        assert_eq!(merged_state(&job("working", None), None), "working");
    }

    #[test]
    fn merged_state_loopish_success_stays_live() {
        // lDe: a routine-backed success job falls through to blocked/working.
        let mut j = job("done", Some("blocked"));
        j.routine = Some(json!("nightly"));
        assert_eq!(merged_state(&j, None), "blocked");
        let mut k = job("done", None);
        k.intent = Some("/loop 5m check builds".to_string());
        assert_eq!(merged_state(&k, None), "working");
        // …but a FAILED loopish job is still terminal.
        let mut f = job("failed", None);
        f.routine = Some(json!("nightly"));
        assert_eq!(merged_state(&f, None), "failed");
    }

    #[test]
    fn job_origin_cwd_strips_managed_worktree() {
        let mut j = JobState {
            cwd: Some(format!(
                "/home/u/proj/{}/worktrees/fix-thing",
                branding::DOT_DIR
            )),
            ..JobState::default()
        };
        assert_eq!(job_origin_cwd(&j), "/home/u/proj");
        j.origin_cwd = Some("/home/u/elsewhere".to_string());
        assert_eq!(job_origin_cwd(&j), "/home/u/elsewhere");
    }

    #[test]
    fn cwd_filter_keeps_root_and_descendants_only() {
        let root = Path::new("/home/u/proj");
        assert!(cwd_matches(Some(root), "/home/u/proj"));
        assert!(cwd_matches(Some(root), "/home/u/proj/sub"));
        assert!(!cwd_matches(Some(root), "/home/u/other"));
        assert!(!cwd_matches(Some(root), "/home/u"));
        assert!(cwd_matches(None, "/anywhere"));
    }

    fn live(pid: i32, kind: &str, started_at: i64) -> LiveSessionRecord {
        LiveSessionRecord {
            pid,
            session_id: Some(format!("sess-{pid}")),
            cwd: "/home/u/proj".to_string(),
            started_at,
            proc_start: None,
            version: None,
            peer_protocol: None,
            kind: kind.to_string(),
            job_id: None,
            entrypoint: None,
            name: Some(format!("name-{pid}")),
            name_source: None,
            status: Some("idle".to_string()),
            waiting_for: None,
            updated_at: None,
            status_updated_at: None,
            name_since: None,
            former_names: None,
            messaging_socket_path: None,
            permission_class: None,
        }
    }

    #[test]
    fn build_json_merges_jobs_and_live_in_binary_key_order() {
        let mut worker = live(51658, "bg", 2_000);
        worker.job_id = Some("bc7c6b33".to_string());
        worker.status = Some("busy".to_string());
        let interactive = live(32272, "interactive", 3_000);
        let job_state = JobState {
            state: "working".to_string(),
            tempo: Some("active".to_string()),
            name: Some("token calculation boundary".to_string()),
            session_id: Some("7169-...".to_string()),
            cwd: Some("/home/u/proj/wt".to_string()),
            created_at: Some("2026-07-02T00:00:00.000Z".to_string()),
            ..JobState::default()
        };
        let rows = build_agents_json(
            &[worker, interactive],
            &[("bc7c6b33".to_string(), job_state)],
            None,
            false,
        );
        assert_eq!(rows.len(), 2);
        // Job row: exact key order pid, id, cwd, kind, startedAt, sessionId,
        // name, status, state (preserve_order keeps the Map faithful).
        let keys: Vec<&str> = rows[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "pid",
                "id",
                "cwd",
                "kind",
                "startedAt",
                "sessionId",
                "name",
                "status",
                "state"
            ]
        );
        assert_eq!(rows[0]["kind"], "background");
        assert_eq!(rows[0]["state"], "working");
        assert_eq!(rows[0]["status"], "busy");
        // Live-only interactive row: pid, cwd, kind, startedAt, sessionId,
        // name, status.
        let keys: Vec<&str> = rows[1]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "pid",
                "cwd",
                "kind",
                "startedAt",
                "sessionId",
                "name",
                "status"
            ]
        );
        assert_eq!(rows[1]["kind"], "interactive");
    }

    #[test]
    fn build_json_default_hides_completed_all_shows_them() {
        let done = JobState {
            state: "done".to_string(),
            tempo: Some("idle".to_string()),
            session_id: Some("s".to_string()),
            cwd: Some("/p".to_string()),
            created_at: Some("2026-07-01T00:00:00.000Z".to_string()),
            ..JobState::default()
        };
        let jobs = vec![("aaaa1111".to_string(), done)];
        assert!(build_agents_json(&[], &jobs, None, false).is_empty());
        let all = build_agents_json(&[], &jobs, None, true);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0]["state"], "done");
        // Workerless job rows carry no pid key at all.
        assert!(all[0].get("pid").is_none());
    }

    #[test]
    fn build_json_sorts_by_started_at_ascending() {
        let a = live(2, "interactive", 5_000);
        let b = live(1, "interactive", 1_000);
        let rows = build_agents_json(&[a, b], &[], None, false);
        assert_eq!(rows[0]["startedAt"], 1_000);
        assert_eq!(rows[1]["startedAt"], 5_000);
    }

    #[test]
    fn registration_writes_and_drop_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = SessionRegistration::register(tmp.path(), Some("sid-1"), Some("proj"));
        let path = sessions_dir(tmp.path()).join(format!("{}.json", std::process::id()));
        assert!(path.exists());
        // Own pid is alive → the reader keeps the record.
        let recs = read_live_sessions(&sessions_dir(tmp.path()));
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].session_id.as_deref(), Some("sid-1"));
        assert_eq!(recs[0].kind, "interactive");
        drop(reg);
        assert!(!path.exists());
    }

    #[test]
    fn update_status_rewrites_record_and_bumps_both_timestamps() {
        // Binary `mvn` (@222989611): patch always carries `updatedAt`; the
        // status key is always present in the patch so `statusUpdatedAt`
        // bumps too — but only on a real (status, waitingFor) change (the
        // caller's useEffect deps), mirrored by the no-op guard.
        let tmp = tempfile::tempdir().unwrap();
        let reg = SessionRegistration::register(tmp.path(), Some("sid-1"), Some("proj"));
        let path = sessions_dir(tmp.path()).join(format!("{}.json", std::process::id()));
        let read = || {
            serde_json::from_str::<LiveSessionRecord>(&std::fs::read_to_string(&path).unwrap())
                .unwrap()
        };
        let before = read();
        assert_eq!(before.status.as_deref(), Some("idle"));

        reg.update_status("busy", None);
        let busy = read();
        assert_eq!(busy.status.as_deref(), Some("busy"));
        assert!(busy.status_updated_at >= before.status_updated_at);

        // waiting carries the binary's waitingFor reason.
        reg.update_status("waiting", Some("permission prompt"));
        let waiting = read();
        assert_eq!(waiting.status.as_deref(), Some("waiting"));
        assert_eq!(waiting.waiting_for.as_deref(), Some("permission prompt"));

        // Leaving waiting clears waitingFor.
        reg.update_status("idle", None);
        let idle = read();
        assert_eq!(idle.status.as_deref(), Some("idle"));
        assert_eq!(idle.waiting_for, None);

        // No-op guard: same (status, waitingFor) pair leaves the file bytes
        // (and timestamps) untouched.
        let stamped = read();
        reg.update_status("idle", None);
        let unchanged = read();
        assert_eq!(unchanged.status_updated_at, stamped.status_updated_at);
        assert_eq!(unchanged.updated_at, stamped.updated_at);
        drop(reg);
        assert!(!path.exists());
    }

    #[test]
    fn read_live_sessions_reaps_dead_pids() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = sessions_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        // A pid that can't be alive (kernel-reserved huge pid on macOS/Linux
        // test hosts).
        let rec = live(i32::MAX - 7, "interactive", 1);
        std::fs::write(dir.join("weird.json"), serde_json::to_string(&rec).unwrap()).unwrap();
        let recs = read_live_sessions(&dir);
        assert!(recs.is_empty());
        assert!(!dir.join("weird.json").exists());
    }

    #[test]
    fn read_jobs_reads_state_json_per_short_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = jobs_dir(tmp.path());
        std::fs::create_dir_all(dir.join("ad612c16")).unwrap();
        std::fs::write(
            dir.join("ad612c16/state.json"),
            r#"{"state":"blocked","tempo":"blocked","name":"wf audit","sessionId":"ad61-1","cwd":"/p","createdAt":"2026-06-26T03:31:58.390Z"}"#,
        )
        .unwrap();
        let jobs = read_jobs(&dir);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0, "ad612c16");
        assert_eq!(merged_state(&jobs[0].1, None), "blocked");
        assert_eq!(
            parse_created_at_ms(jobs[0].1.created_at.as_deref()),
            1_782_444_718_390
        );
    }

    // ---- job WRITER (`--bg` dispatch) ----------------------------------

    fn fresh_bg_job<'a>(
        session_id: &'a str,
        cwd: &'a str,
        created_at: &'a str,
        intent: &'a str,
        prompt: &'a str,
        respawn: &'a [String],
    ) -> JobStateWrite<'a> {
        JobStateWrite {
            state: "working",
            tempo: Some("active"),
            name: None,
            session_id: Some(session_id),
            cwd: Some(cwd),
            origin_cwd: Some(cwd),
            created_at: Some(created_at),
            intent: Some(intent),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: respawn,
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some(prompt),
            detail: None,
            worker_pid: None,
        }
    }

    #[test]
    fn write_job_state_round_trips_through_read_jobs_in_pinned_key_order() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let respawn: Vec<String> = Vec::new();
        let job = fresh_bg_job(
            "7169-abcd",
            "/home/u/proj",
            "2026-07-04T00:00:00.000Z",
            "port the daemon",
            "port the daemon supervisor",
            &respawn,
        );
        write_job_state(home, "bc7c6b33", &job).unwrap();

        // Raw on-disk bytes: pinned key ORDER + the design's pinned substrings.
        let raw = std::fs::read_to_string(jobs_dir(home).join("bc7c6b33/state.json")).unwrap();
        assert!(raw.contains(r#""state":"working""#), "{raw}");
        assert!(raw.contains(r#""template":"bg""#), "{raw}");
        assert!(raw.contains(r#""backend":"daemon""#), "{raw}");
        // key order: state before tempo before sessionId before createdAt before
        // template before respawnFlags.
        let idx = |k: &str| raw.find(k).unwrap();
        assert!(idx(r#""state""#) < idx(r#""tempo""#));
        assert!(idx(r#""tempo""#) < idx(r#""sessionId""#));
        assert!(idx(r#""sessionId""#) < idx(r#""createdAt""#));
        assert!(idx(r#""createdAt""#) < idx(r#""template""#));
        assert!(idx(r#""template""#) < idx(r#""respawnFlags""#));

        // The reader picks it up and merges it into `agents --json`.
        let jobs = read_jobs(&jobs_dir(home));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0, "bc7c6b33");
        let rows = build_agents_json(&[], &jobs, None, false);
        assert_eq!(
            rows.len(),
            1,
            "workerless working row survives default filter"
        );
        assert_eq!(rows[0]["id"], "bc7c6b33");
        assert_eq!(rows[0]["state"], "working");
        assert_eq!(rows[0]["kind"], "background");
        // Label falls back intent → sanitize_name; startedAt from createdAt ≠ 0.
        assert_eq!(rows[0]["name"], "port the daemon");
        assert_ne!(
            rows[0]["startedAt"].as_i64().unwrap(),
            0,
            "RFC3339-millis-Z createdAt parses to a non-zero epoch"
        );
    }

    #[test]
    fn write_job_state_is_atomic_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let respawn: Vec<String> = Vec::new();
        let job = fresh_bg_job("s", "/p", "2026-07-04T00:00:00.000Z", "i", "p", &respawn);
        write_job_state(home, "aaaa1111", &job).unwrap();
        // No torn temp file left behind.
        let dir = jobs_dir(home).join("aaaa1111");
        assert!(dir.join("state.json").exists());
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "temp file renamed away");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            let file_mode = std::fs::metadata(dir.join("state.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
            assert_eq!(file_mode, 0o600);
        }
    }

    #[test]
    fn read_job_reads_single_short() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let respawn: Vec<String> = Vec::new();
        let job = fresh_bg_job(
            "sid-1",
            "/work",
            "2026-07-04T00:00:00.000Z",
            "intent",
            "do the thing",
            &respawn,
        );
        write_job_state(home, "bc7c6b33", &job).unwrap();
        let got = read_job(home, "bc7c6b33").expect("job present");
        assert_eq!(got.state, "working");
        assert_eq!(got.initial_prompt.as_deref(), Some("do the thing"));
        assert_eq!(got.cwd.as_deref(), Some("/work"));
        assert_eq!(got.session_id.as_deref(), Some("sid-1"));
        assert!(read_job(home, "nope0000").is_none());
    }

    #[test]
    fn update_job_state_records_worker_pid_then_marks_terminal() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let respawn: Vec<String> = Vec::new();
        let job = fresh_bg_job(
            "sid-1",
            "/work",
            "2026-07-04T00:00:00.000Z",
            "port the daemon",
            "port the daemon supervisor",
            &respawn,
        );
        write_job_state(home, "bc7c6b33", &job).unwrap();

        // Supervisor records the spawned worker's pid; state stays "working"
        // (tempo preserved "active") so the job is NOT terminal yet.
        update_job_state(home, "bc7c6b33", "working", Some(4242)).unwrap();
        let mid = read_job(home, "bc7c6b33").unwrap();
        assert_eq!(mid.state, "working");
        assert_eq!(mid.tempo.as_deref(), Some("active"));
        assert_eq!(mid.worker_pid, Some(4242));
        assert!(!job_is_terminal(&mid), "working job is not terminal");
        // Pinned fields survive the read-modify-write.
        assert_eq!(mid.template.as_deref(), Some("bg"));
        assert_eq!(mid.backend.as_deref(), Some("daemon"));
        assert_eq!(
            mid.initial_prompt.as_deref(),
            Some("port the daemon supervisor")
        );

        // Worker completes: terminal "done" + tempo forced off "active" + pid
        // cleared. merged_state now reports the terminal outcome.
        update_job_state(home, "bc7c6b33", "done", None).unwrap();
        let done = read_job(home, "bc7c6b33").unwrap();
        assert_eq!(done.state, "done");
        assert_ne!(done.tempo.as_deref(), Some("active"));
        assert_eq!(done.worker_pid, None);
        assert!(job_is_terminal(&done));
        assert_eq!(merged_state(&done, None), "done");

        // The pinned prefix key order is still honored on disk.
        let raw = std::fs::read_to_string(jobs_dir(home).join("bc7c6b33/state.json")).unwrap();
        let idx = |k: &str| raw.find(k).unwrap();
        assert!(idx(r#""state""#) < idx(r#""tempo""#));
        assert!(idx(r#""template""#) < idx(r#""respawnFlags""#));

        // A missing job errors rather than fabricating a row.
        assert!(update_job_state(home, "missing0", "done", None).is_err());
    }

    #[test]
    fn update_job_state_failed_is_terminal() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let respawn: Vec<String> = Vec::new();
        let job = fresh_bg_job("s", "/w", "2026-07-04T00:00:00.000Z", "i", "p", &respawn);
        write_job_state(home, "aaaa1111", &job).unwrap();
        update_job_state(home, "aaaa1111", "failed", None).unwrap();
        let f = read_job(home, "aaaa1111").unwrap();
        assert!(job_is_terminal(&f));
        assert_eq!(merged_state(&f, None), "failed");
        // The detail-less writer never emits a `detail` key (fresh-job shape).
        let raw = std::fs::read_to_string(jobs_dir(home).join("aaaa1111/state.json")).unwrap();
        assert!(!raw.contains(r#""detail""#));
    }

    #[test]
    fn update_job_state_with_detail_stamps_the_detail_line() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let respawn: Vec<String> = Vec::new();
        let job = fresh_bg_job("s", "/w", "2026-07-04T00:00:00.000Z", "i", "p", &respawn);
        write_job_state(home, "beef0001", &job).unwrap();
        update_job_state_with_detail(
            home,
            "beef0001",
            "failed",
            None,
            "working directory no longer exists or is not accessible: /w",
        )
        .unwrap();
        let f = read_job(home, "beef0001").unwrap();
        assert!(job_is_terminal(&f));
        assert_eq!(
            f.detail.as_deref(),
            Some("working directory no longer exists or is not accessible: /w")
        );
        assert_eq!(f.worker_pid, None);
    }

    #[test]
    fn mint_short_id_is_eight_lowercase_hex() {
        let tmp = tempfile::tempdir().unwrap();
        let short = mint_short_id(tmp.path());
        assert_eq!(short.len(), 8);
        assert!(
            short
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "8 lowercase hex: {short}"
        );
    }

    #[test]
    fn mint_short_id_skips_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(jobs_dir(home).join("aaaa1111")).unwrap();
        // Generator yields the colliding id first (skipped), then a free one.
        let ids = ["aaaa1111", "bbbb2222"];
        let mut n = 0usize;
        let short = mint_short_id_with(home, &mut || {
            let id = ids[n].to_string();
            n += 1;
            id
        });
        assert_eq!(short, "bbbb2222");
    }

    #[test]
    fn register_bg_carries_kind_and_job_id() {
        let tmp = tempfile::tempdir().unwrap();
        let reg =
            SessionRegistration::register_bg(tmp.path(), Some("sid-9"), Some("proj"), "bc7c6b33");
        let path = sessions_dir(tmp.path()).join(format!("{}.json", std::process::id()));
        let rec: LiveSessionRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(rec.kind, "bg");
        assert_eq!(rec.job_id.as_deref(), Some("bc7c6b33"));
        assert_eq!(rec.session_id.as_deref(), Some("sid-9"));
        drop(reg);
        assert!(!path.exists());
    }
}
