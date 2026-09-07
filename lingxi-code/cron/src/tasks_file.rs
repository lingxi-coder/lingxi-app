//! Single-file scheduled-task persistence — 1:1 with claude-code
//! `utils/cronTasks.ts`.
//!
//! claude-code stores ALL scheduled (durable) cron jobs in ONE project-relative
//! file `<projectRoot>/.lingxi/scheduled_tasks.json` shaped
//! `{ "tasks": [ CronTask, … ] }`, NOT one file per job. This module owns the
//! single authoritative [`CronTask`] / [`ScheduledTasks`] definition (camelCase
//! JSON, epoch **milliseconds**) plus path + (de)serialization helpers, shared
//! by every reader/writer:
//!
//! - the `CronCreate` / `CronList` / `CronDelete` tools (in `tool-cron`, which
//!   depends on this crate), and
//! - the live [`crate::scheduler::CronScheduler`] (same crate), which writes
//!   `lastFiredAt` back after each recurring fire.
//!
//! Runtime-only fields are NEVER persisted: `durable` (everything on disk is
//! durable by definition) and `agentId` (a teammate-routing handle). There is
//! also NO persisted next-fire field — claude-code computes the next fire time
//! at runtime from the cron string + `lastFiredAt ?? createdAt`
//! ([`crate::schedule::CronExpression::next_match_after`]).

use platform_api::{FileSystem, FlockGuard, FsError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Project-relative `.claude` subdir holding the single tasks file.
pub const CLAUDE_DIR: &str = branding::DOT_DIR;
/// Single-file name (claude-code `SCHEDULED_TASKS_FILE`).
pub const SCHEDULED_TASKS_FILE: &str = "scheduled_tasks.json";
/// Lock-file name beside the tasks file (claude-code `scheduled_tasks.lock`).
pub const SCHEDULED_TASKS_LOCK: &str = "scheduled_tasks.lock";

/// One persisted scheduled cron job — the EXACT on-disk shape of a claude-code
/// `CronTask`, in the canonical key order
/// `{ id, cron, prompt, createdAt, lastFiredAt?, recurring?, permanent? }`.
///
/// `createdAt` / `lastFiredAt` are epoch **milliseconds** (claude-code
/// `Date.now()`). Optional fields are OMITTED when absent
/// (`skip_serializing_if`), matching claude-code's optional `lastFiredAt?` /
/// `recurring?` / `permanent?`. The runtime-only `durable` and `agentId` fields
/// are intentionally absent — they never round-trip through disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronTask {
    /// Stable 9-char job id (`d` + 8 base36 chars).
    pub id: String,
    /// 5-field cron expression (local time).
    pub cron: String,
    /// Prompt enqueued at each fire time.
    pub prompt: String,
    /// Creation time, epoch **milliseconds**.
    pub created_at: u64,
    /// Last fire time, epoch **milliseconds**. Omitted until the job first
    /// fires; written back by the scheduler after each recurring fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<u64>,
    /// `true` = fire on every cron match until deleted / auto-expired; absent or
    /// `false` = one-shot. Omitted when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring: Option<bool>,
    /// System task marker — not settable via the tool, but preserved verbatim on
    /// read/write round-trips. Omitted when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permanent: Option<bool>,
    /// Optional absolute expiration, epoch milliseconds. Absent means indefinite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Conversation that owns this task, when created through Desktop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// claude-code `ensureClaudeRuntimeFilesExcluded` — the runtime files the
/// scheduler creates inside the project are appended to `.git/info/exclude`
/// once per process, under a marker line, so they never show up as untracked.
/// (`.claude/` → `.lingxi/`; the marker is `# lingxi-code-runtime`.)
pub const RUNTIME_EXCLUDE_MARKER: &str = "# lingxi-code-runtime";
/// The patterns appended after [`RUNTIME_EXCLUDE_MARKER`].
pub const RUNTIME_EXCLUDE_PATTERNS: [&str; 10] = [
    "**/.lingxi/scheduled_tasks.lock",
    "**/.lingxi/scheduled_tasks.json",
    "**/.lingxi/routines/.state/",
    "**/.lingxi/worktrees/",
    "**/.lingxi/checkpoints/",
    "**/.lingxi/mailbox/",
    "**/.lingxi/agent-registry.json",
    "**/.lingxi/agent-memory-local",
    "**/.lingxi/first-run",
    "**/.lingxi/assistant-daemon-state.json",
];

static RUNTIME_EXCLUDE_ENSURED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// Resolve the git directory that owns `project_root` (a `.git` directory, or
/// a `.git` file pointing at a worktree's git dir, whose `commondir` is the
/// shared repository) — claude-code `Nw(dir)` then `yL(gitDir) ?? gitDir`.
fn git_common_dir(project_root: &Path) -> Option<std::path::PathBuf> {
    let dot_git = project_root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else if dot_git.is_file() {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let target = pointer.trim().strip_prefix("gitdir:")?.trim();
        let target = Path::new(target);
        if target.is_absolute() {
            target.to_path_buf()
        } else {
            project_root.join(target)
        }
    } else {
        return None;
    };
    let common = std::fs::read_to_string(git_dir.join("commondir")).ok().map(|s| {
        let s = s.trim();
        let p = Path::new(s);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            git_dir.join(p)
        }
    });
    Some(common.unwrap_or(git_dir))
}

/// Append the runtime exclude block to `<git dir>/info/exclude` unless the
/// marker is already present. Best effort and idempotent per process
/// (claude-code `ensureClaudeRuntimeFilesExcluded`, logged on failure only).
pub fn ensure_runtime_files_excluded(project_root: &Path) {
    if !RUNTIME_EXCLUDE_ENSURED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(project_root.to_path_buf())
    {
        return;
    }
    let Some(git_dir) = git_common_dir(project_root) else {
        return;
    };
    let info = git_dir.join("info");
    let exclude = info.join("exclude");
    let existing = match std::fs::read_to_string(&exclude) {
        Ok(existing) => {
            if existing.contains(RUNTIME_EXCLUDE_MARKER) {
                return;
            }
            existing
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Err(error) = std::fs::create_dir_all(&info) {
                tracing::warn!("ensureRuntimeFilesExcluded: {error}");
                return;
            }
            String::new()
        }
        Err(error) => {
            tracing::warn!("ensureRuntimeFilesExcluded: {error}");
            return;
        }
    };
    let lead = if !existing.is_empty() && !existing.ends_with('\n') { "\n" } else { "" };
    let mut block = String::from(RUNTIME_EXCLUDE_MARKER);
    for pattern in RUNTIME_EXCLUDE_PATTERNS {
        block.push('\n');
        block.push_str(pattern);
    }
    block.push('\n');
    use std::io::Write as _;
    let appended = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&exclude)
        .and_then(|mut f| f.write_all(format!("{lead}{block}").as_bytes()));
    if let Err(error) = appended {
        tracing::warn!("ensureRuntimeFilesExcluded: {error}");
    }
}

/// The whole on-disk document: `{ "tasks": [ … ] }`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTasks {
    /// Every durable scheduled cron job.
    pub tasks: Vec<CronTask>,
    /// Entries [`parse_tasks`] could NOT model — an unparseable cron, a
    /// wrong-typed field, a record a newer writer produced. They are skipped for
    /// scheduling exactly as claude-code skips them, but carried here verbatim
    /// so that a read-modify-write ([`serialize_tasks`] after a `retain`/`push`)
    /// puts them back instead of silently deleting a record the user can still
    /// repair by hand. Every write path in the port re-serializes the whole
    /// document, so without this a malformed sibling evaporates the first time
    /// any OTHER task fires, is deleted, or is created.
    #[serde(skip)]
    pub unmodeled: Vec<UnmodeledTask>,
}

/// One entry [`parse_tasks`] skipped, kept with its position in the original
/// `tasks` array so [`serialize_tasks`] can splice it back where it was.
#[derive(Debug, Clone, PartialEq)]
pub struct UnmodeledTask {
    /// Index of this entry in the `tasks` array it was read from.
    pub index: usize,
    /// The entry exactly as it appeared on disk.
    pub raw: serde_json::Value,
}

/// Absolute path to the single tasks file for `project_root`
/// (`<project_root>/.lingxi/scheduled_tasks.json`).
#[must_use]
pub fn scheduled_tasks_path(project_root: &Path) -> PathBuf {
    project_root.join(CLAUDE_DIR).join(SCHEDULED_TASKS_FILE)
}

/// Absolute path to the lock file beside the tasks file
/// (`<project_root>/.lingxi/scheduled_tasks.lock`).
#[must_use]
pub fn scheduled_tasks_lock_path(project_root: &Path) -> PathBuf {
    project_root.join(CLAUDE_DIR).join(SCHEDULED_TASKS_LOCK)
}

/// Root-relative path used by hardened filesystem operations.
#[must_use]
pub fn scheduled_tasks_relative_path() -> PathBuf {
    PathBuf::from(CLAUDE_DIR).join(SCHEDULED_TASKS_FILE)
}

/// Root-relative advisory-lock path used for every scheduled-task mutation.
#[must_use]
pub fn scheduled_tasks_lock_relative_path() -> PathBuf {
    PathBuf::from(CLAUDE_DIR).join(SCHEDULED_TASKS_LOCK)
}

/// Recover the project root from a canonical scheduled-tasks path.
#[must_use]
pub fn project_root_from_tasks_path(tasks_file: &Path) -> Option<&Path> {
    let state_dir = tasks_file.parent()?;
    if state_dir.file_name()? != std::ffi::OsStr::new(CLAUDE_DIR)
        || tasks_file.file_name()? != std::ffi::OsStr::new(SCHEDULED_TASKS_FILE)
    {
        return None;
    }
    state_dir.parent().map(|root| {
        if root.as_os_str().is_empty() {
            Path::new(".")
        } else {
            root
        }
    })
}

/// Acquire the cross-process lock for a scheduled-task read-modify-write.
pub async fn lock_scheduled_tasks(
    fs: &dyn FileSystem,
    project_root: &Path,
) -> Result<Box<dyn FlockGuard>, FsError> {
    fs.flock_exclusive_rooted(project_root, &scheduled_tasks_lock_relative_path())
        .await
}

/// Read the scheduled-task document without following project-local symlinks.
pub async fn read_tasks_body(fs: &dyn FileSystem, project_root: &Path) -> Result<String, FsError> {
    fs.read_file_rooted_no_follow(project_root, &scheduled_tasks_relative_path())
        .await
        .map(|content| content.content)
}

/// Atomically replace the scheduled-task document without following
/// project-local symlinks.
pub async fn write_tasks_body(
    fs: &dyn FileSystem,
    project_root: &Path,
    body: &str,
) -> Result<(), FsError> {
    fs.write_file_rooted_atomic(project_root, &scheduled_tasks_relative_path(), body)
        .await
}

/// Parse a tasks-file body into [`ScheduledTasks`] the way claude-code's
/// `loadScheduledTasks` does: a missing/empty/garbage body or a non-array
/// `tasks` yields an empty document, and each entry is checked on its own — an
/// entry missing a string `id`/`cron`/`prompt` or a numeric `createdAt` is
/// skipped (`[ScheduledTasks] skipping malformed task: …`), and one whose cron
/// does not parse is skipped (`[ScheduledTasks] skipping task ${id} with invalid
/// cron '…'`) — so one bad record never disables the healthy ones.
#[must_use]
pub fn parse_tasks(body: &str) -> ScheduledTasks {
    parse_tasks_strict(body).unwrap_or_default()
}

/// [`parse_tasks`] for the scheduler's authoritative reads: per-entry tolerance
/// is identical, but a body that is not a JSON object with a `tasks` array is an
/// error rather than an empty document, so corrupt durable state is never
/// reinterpreted as "no tasks" on a firing path.
pub fn parse_tasks_strict(body: &str) -> Result<ScheduledTasks, String> {
    let doc = serde_json::from_str::<serde_json::Value>(body).map_err(|e| e.to_string())?;
    let Some(entries) = doc.get("tasks").and_then(serde_json::Value::as_array) else {
        return Err("expected an object with a `tasks` array".to_string());
    };
    let mut unmodeled: Vec<UnmodeledTask> = Vec::new();
    let tasks = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let mut skip = |entry: &serde_json::Value| {
                unmodeled.push(UnmodeledTask {
                    index,
                    raw: entry.clone(),
                });
            };
            let (Some(id), Some(cron), Some(prompt), Some(created_at)) = (
                entry.get("id").and_then(serde_json::Value::as_str),
                entry.get("cron").and_then(serde_json::Value::as_str),
                entry.get("prompt").and_then(serde_json::Value::as_str),
                entry.get("createdAt").and_then(serde_json::Value::as_f64),
            ) else {
                tracing::warn!("[ScheduledTasks] skipping malformed task: {entry}");
                skip(entry);
                return None;
            };
            if crate::schedule::parse_cron(cron).is_err() {
                tracing::warn!("[ScheduledTasks] skipping task {id} with invalid cron '{cron}'");
                skip(entry);
                return None;
            }
            // A present-but-non-positive `expiresAt` is NOT an expiry instant:
            // `0` / a negative / `NaN` would normalise to `Some(0)`, which every
            // reader tests as `expiry <= now` and deletes the task. Treat those
            // the way `lastFiredAt` is treated downstream — as absent.
            let ms = |v: f64| if v.is_finite() && v > 0.0 { v as u64 } else { 0 };
            Some(CronTask {
                id: id.to_string(),
                cron: cron.to_string(),
                prompt: prompt.to_string(),
                created_at: ms(created_at),
                last_fired_at: entry
                    .get("lastFiredAt")
                    .and_then(serde_json::Value::as_f64)
                    .map(ms),
                // claude-code normalises `recurring` / `permanent` to present-only-when-true.
                recurring: entry
                    .get("recurring")
                    .and_then(serde_json::Value::as_bool)
                    .filter(|v| *v),
                permanent: entry
                    .get("permanent")
                    .and_then(serde_json::Value::as_bool)
                    .filter(|v| *v),
                expires_at: entry
                    .get("expiresAt")
                    .and_then(serde_json::Value::as_f64)
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .map(ms),
                session_id: entry
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    Ok(ScheduledTasks { tasks, unmodeled })
}

/// Serialize [`ScheduledTasks`] to the EXACT bytes claude-code writes:
/// `JSON.stringify(body, null, 2) + '\n'` — pretty 2-space indent plus a single
/// trailing newline.
///
/// Entries the parser could not model ([`ScheduledTasks::unmodeled`]) are
/// spliced back at their original positions, so a read-modify-write preserves
/// records this port skips rather than deleting them. With no such entries this
/// is byte-identical to serializing the struct directly (`serde_json` is built
/// with `preserve_order`, so the field order of every task is unchanged).
#[must_use]
pub fn serialize_tasks(tasks: &ScheduledTasks) -> String {
    let fallback = || String::from("{\n  \"tasks\": []\n}");
    let mut s = if tasks.unmodeled.is_empty() {
        // `serde_json::to_string_pretty` uses a 2-space indent (matching
        // `JSON.stringify(_, null, 2)`).
        serde_json::to_string_pretty(tasks).unwrap_or_else(|_| fallback())
    } else {
        let mut entries: Vec<serde_json::Value> = tasks
            .tasks
            .iter()
            .map(|task| serde_json::to_value(task).unwrap_or(serde_json::Value::Null))
            .collect();
        // Ascending by recorded index (the order `parse_tasks_strict` collects
        // them in), clamped because the modeled tasks may have shrunk or grown.
        for skipped in &tasks.unmodeled {
            let at = skipped.index.min(entries.len());
            entries.insert(at, skipped.raw.clone());
        }
        serde_json::to_string_pretty(&serde_json::json!({ "tasks": entries }))
            .unwrap_or_else(|_| fallback())
    };
    // The trailing newline claude-code adds.
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn path_is_project_relative() {
        let p = scheduled_tasks_path(Path::new("/proj"));
        assert_eq!(p, Path::new("/proj/.lingxi/scheduled_tasks.json"));
        let l = scheduled_tasks_lock_path(Path::new("/proj"));
        assert_eq!(l, Path::new("/proj/.lingxi/scheduled_tasks.lock"));
        assert_eq!(
            project_root_from_tasks_path(Path::new(".lingxi/scheduled_tasks.json")),
            Some(Path::new("."))
        );
        assert_eq!(
            project_root_from_tasks_path(Path::new("/proj/elsewhere/tasks.json")),
            None
        );
    }

    #[test]
    fn round_trips_camel_case_ms_and_omits_absent_optionals() {
        let doc = ScheduledTasks {
            tasks: vec![CronTask {
                id: "d12345678".into(),
                cron: "0 9 * * *".into(),
                prompt: "hi".into(),
                created_at: 1_700_000_000_000,
                last_fired_at: None,
                recurring: Some(true),
                permanent: None,
                expires_at: None,
                session_id: None,
            }],
            ..Default::default()
        };
        let s = serialize_tasks(&doc);
        // camelCase keys, epoch-ms number, trailing newline, 2-space indent.
        assert!(s.ends_with("}\n"));
        assert!(s.contains("\"createdAt\": 1700000000000"));
        assert!(s.contains("\"recurring\": true"));
        // Absent optionals are omitted entirely.
        assert!(!s.contains("lastFiredAt"));
        assert!(!s.contains("permanent"));
        // No runtime-only fields ever appear.
        assert!(!s.contains("durable"));
        assert!(!s.contains("agentId"));
        // Round-trips exactly.
        assert_eq!(parse_tasks(&s), doc);
    }

    #[test]
    fn key_order_matches_claude_code() {
        let doc = ScheduledTasks {
            tasks: vec![CronTask {
                id: "d00000000".into(),
                cron: "* * * * *".into(),
                prompt: "p".into(),
                created_at: 1,
                last_fired_at: Some(2),
                recurring: Some(true),
                permanent: Some(true),
                expires_at: None,
                session_id: None,
            }],
            ..Default::default()
        };
        let s = serialize_tasks(&doc);
        let id_at = s.find("\"id\"").unwrap();
        let cron_at = s.find("\"cron\"").unwrap();
        let prompt_at = s.find("\"prompt\"").unwrap();
        let created_at = s.find("\"createdAt\"").unwrap();
        let fired_at = s.find("\"lastFiredAt\"").unwrap();
        let recurring_at = s.find("\"recurring\"").unwrap();
        let permanent_at = s.find("\"permanent\"").unwrap();
        assert!(id_at < cron_at);
        assert!(cron_at < prompt_at);
        assert!(prompt_at < created_at);
        assert!(created_at < fired_at);
        assert!(fired_at < recurring_at);
        assert!(recurring_at < permanent_at);
    }

    #[test]
    fn missing_or_garbage_body_is_empty() {
        assert_eq!(parse_tasks(""), ScheduledTasks::default());
        assert_eq!(parse_tasks("not json"), ScheduledTasks::default());
        assert_eq!(parse_tasks("{}"), ScheduledTasks::default());
        assert_eq!(parse_tasks(r#"{"tasks":"nope"}"#), ScheduledTasks::default());
    }

    // PARITY 2.1.263 `Q7e`: malformed entries and invalid crons are skipped
    // individually; the healthy entries still load.
    #[test]
    fn malformed_entries_are_skipped_one_by_one() {
        let body = r#"{"tasks":[
            {"id":"bad1","cron":"* * * * *","createdAt":1},
            {"id":42,"cron":"* * * * *","prompt":"p","createdAt":1},
            {"id":"bad3","cron":"* * * * *","prompt":"p","createdAt":"1"},
            {"id":"badcron","cron":"61 * * * *","prompt":"p","createdAt":1},
            {"id":"ok","cron":"*/5 * * * *","prompt":"p","createdAt":1.9,"lastFiredAt":2,"recurring":false,"permanent":false,"extra":true}
        ]}"#;
        let doc = parse_tasks(body);
        assert_eq!(doc.tasks.len(), 1);
        let ok = &doc.tasks[0];
        assert_eq!(ok.id, "ok");
        assert_eq!(ok.created_at, 1, "JS numbers are accepted, truncated to ms");
        assert_eq!(ok.last_fired_at, Some(2));
        assert_eq!(ok.recurring, None, "`recurring: false` normalises to absent");
        assert_eq!(ok.permanent, None);
        assert_eq!(doc.unmodeled.len(), 4, "the four skipped entries are kept");
        assert_eq!(
            doc.unmodeled.iter().map(|u| u.index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    // A skipped entry must SURVIVE a read-modify-write. Every write path in the
    // port re-serializes the whole document, so without this a record the parser
    // cannot model is silently deleted the first time any OTHER task fires, is
    // deleted, or is created — and the user loses a task they could have
    // repaired by hand.
    #[test]
    fn a_read_modify_write_preserves_entries_the_parser_skips() {
        let body = r#"{
  "tasks": [
    {
      "id": "badcron",
      "cron": "0 9 * * MON",
      "prompt": "keep me",
      "createdAt": 1
    },
    {
      "id": "ok",
      "cron": "*/5 * * * *",
      "prompt": "p",
      "createdAt": 1
    }
  ]
}
"#;
        let mut doc = parse_tasks(body);
        assert_eq!(doc.tasks.len(), 1, "the named-weekday cron is skipped");
        // Delete the healthy task, exactly as a one-shot auto-delete does.
        doc.tasks.retain(|t| t.id != "ok");
        let written = serialize_tasks(&doc);
        assert!(
            written.contains("\"id\": \"badcron\"") && written.contains("0 9 * * MON"),
            "the skipped entry must still be on disk:\n{written}"
        );
        // It is still skipped for scheduling, and still preserved.
        let reread = parse_tasks(&written);
        assert!(reread.tasks.is_empty());
        assert_eq!(reread.unmodeled.len(), 1);
    }

    // `expiresAt: 0` (and a negative / non-finite one) is not an expiry instant.
    // Normalising it to `Some(0)` makes every reader test `expiry <= now` and
    // delete a healthy task.
    #[test]
    fn a_non_positive_expires_at_reads_as_absent_not_as_already_expired() {
        for raw in ["0", "-1", "-0.5"] {
            let body = format!(
                r#"{{"tasks":[{{"id":"a","cron":"* * * * *","prompt":"p","createdAt":1,"expiresAt":{raw}}}]}}"#
            );
            let doc = parse_tasks(&body);
            assert_eq!(doc.tasks.len(), 1, "expiresAt {raw}");
            assert_eq!(doc.tasks[0].expires_at, None, "expiresAt {raw}");
        }
        let doc = parse_tasks(
            r#"{"tasks":[{"id":"a","cron":"* * * * *","prompt":"p","createdAt":1,"expiresAt":5}]}"#,
        );
        assert_eq!(doc.tasks[0].expires_at, Some(5));
    }

    #[test]
    fn runtime_exclude_block_is_appended_once_and_respects_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join(".git").join("info")).unwrap();
        std::fs::write(root.join(".git/info/exclude"), "*.log").unwrap();
        ensure_runtime_files_excluded(&root);
        let got = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
        let expected = format!(
            "*.log\n{RUNTIME_EXCLUDE_MARKER}\n{}\n",
            RUNTIME_EXCLUDE_PATTERNS.join("\n")
        );
        assert_eq!(got, expected);
        // Second call (same process) and a fresh process with the marker present
        // both leave the file alone.
        ensure_runtime_files_excluded(&root);
        RUNTIME_EXCLUDE_ENSURED.lock().unwrap().remove(&root);
        ensure_runtime_files_excluded(&root);
        assert_eq!(std::fs::read_to_string(root.join(".git/info/exclude")).unwrap(), expected);
        // A worktree `.git` FILE resolves through `gitdir:` and `commondir`.
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let wt_git = root.join(".git/worktrees/wt");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        std::fs::remove_file(root.join(".git/info/exclude")).unwrap();
        ensure_runtime_files_excluded(&wt);
        let got = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
        assert!(got.starts_with(RUNTIME_EXCLUDE_MARKER));
        // No repository at all: a no-op.
        ensure_runtime_files_excluded(&tmp.path().join("nowhere"));
    }

    #[test]
    fn permanent_round_trips() {
        let body = r#"{"tasks":[{"id":"dsys00000","cron":"0 0 * * *","prompt":"sys","createdAt":5,"permanent":true}]}"#;
        let doc = parse_tasks(body);
        assert_eq!(doc.tasks[0].permanent, Some(true));
        // Re-serializing preserves `permanent`.
        let s = serialize_tasks(&doc);
        assert!(s.contains("\"permanent\": true"));
    }
}
