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
}

/// The whole on-disk document: `{ "tasks": [ … ] }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledTasks {
    /// Every durable scheduled cron job.
    pub tasks: Vec<CronTask>,
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

/// Parse a tasks-file body into [`ScheduledTasks`]. A missing/empty/garbage body
/// yields an empty document (claude-code treats an unreadable file as no tasks).
#[must_use]
pub fn parse_tasks(body: &str) -> ScheduledTasks {
    serde_json::from_str(body).unwrap_or_default()
}

/// Serialize [`ScheduledTasks`] to the EXACT bytes claude-code writes:
/// `JSON.stringify(body, null, 2) + '\n'` — pretty 2-space indent plus a single
/// trailing newline.
#[must_use]
pub fn serialize_tasks(tasks: &ScheduledTasks) -> String {
    // `serde_json::to_string_pretty` uses a 2-space indent (matching
    // `JSON.stringify(_, null, 2)`); append the trailing newline claude-code adds.
    let mut s = serde_json::to_string_pretty(tasks).unwrap_or_else(|_| "{\n  \"tasks\": []\n}".into());
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
            }],
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
            }],
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
