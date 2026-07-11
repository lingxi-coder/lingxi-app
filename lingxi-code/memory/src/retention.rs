//! On-disk data-retention sweep — a port of claude-code 2.1.206's `fWu`.
//!
//! The sweep deletes stale session-file entries (older than the retention
//! period) from the config-home housekeeping directories `todos`, `statsig`,
//! and `logs`, mirroring claude's inner loop
//! (`for dir in ["todos","statsig","logs"]: delete entries with mtime < cutoff`).
//!
//! Because it DELETES user data, it is **flag-gated and default-OFF**: it runs
//! only when `LINGXI_RETENTION_SWEEP` is explicitly set to a truthy value, and
//! it never touches anything outside the three named directories. The retention
//! period is claude's `cleanupPeriodDays` (default 30), overridable via
//! `LINGXI_RETENTION_PERIOD_DAYS`.
//!
//! Scope note: the port sweeps the session-file directories only. Claude's `fWu`
//! also runs ~two dozen per-subsystem cleanups plus transcript and worktree
//! cleanup; those are separate subsystems and are not ported here.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// Enable flag — `LINGXI_RETENTION_SWEEP`. Off by default.
pub const RETENTION_ENABLE_ENV: &str = "LINGXI_RETENTION_SWEEP";
/// Period override — `LINGXI_RETENTION_PERIOD_DAYS`.
pub const RETENTION_PERIOD_ENV: &str = "LINGXI_RETENTION_PERIOD_DAYS";
/// claude-code `cleanupPeriodDays` default (`dWu = 30`).
pub const DEFAULT_CLEANUP_PERIOD_DAYS: u64 = 30;

/// Housekeeping directories swept, relative to config-home — claude's
/// `["todos","statsig","logs"]`.
const SWEPT_SUBDIRS: &[&str] = &["todos", "statsig", "logs"];

/// Result of a sweep (claude's `{messages, errors}` accumulator; `messages` is
/// the deleted-entry count).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionReport {
    /// Session-file entries (files or directories) deleted.
    pub session_files_deleted: u64,
    /// Entries that could not be stat'd or removed.
    pub errors: u64,
}

/// Whether the sweep is enabled. **Default OFF** — it runs only when
/// [`RETENTION_ENABLE_ENV`] is a truthy value (`1`/`true`/`yes`/`on`,
/// case-insensitive). Off ⇒ no file is ever touched.
#[must_use]
pub fn retention_sweep_enabled() -> bool {
    std::env::var(RETENTION_ENABLE_ENV)
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// The retention period in days and whether the default was used (claude
/// `cleanupPeriodDays ?? 30`, `usedDefault = cleanupPeriodDays === undefined`).
/// A [`RETENTION_PERIOD_ENV`] value must parse to a positive integer to apply.
#[must_use]
pub fn retention_period_days() -> (u64, bool) {
    match std::env::var(RETENTION_PERIOD_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&d| d > 0)
    {
        Some(days) => (days, false),
        None => (DEFAULT_CLEANUP_PERIOD_DAYS, true),
    }
}

/// Run the sweep rooted at `config_home`, deleting entries in the swept
/// directories whose mtime is older than `now - period`. Pure I/O; performs no
/// gating and emits no telemetry (the caller does both).
///
/// Safety: only the three named subdirectories of `config_home` are read, and
/// only entries strictly older than the cutoff are removed. A missing directory
/// is skipped; an unreadable entry counts as an error, never a deletion.
#[must_use]
pub fn run_retention_sweep_in(config_home: &Path, period: Duration) -> RetentionReport {
    let mut report = RetentionReport::default();
    // A period so large the cutoff underflows ⇒ nothing is old enough ⇒ no-op.
    let Some(cutoff) = SystemTime::now().checked_sub(period) else {
        return report;
    };
    for sub in SWEPT_SUBDIRS {
        let dir = config_home.join(sub);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // absent / unreadable directory → skip
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                report.errors += 1;
                continue;
            };
            let Ok(mtime) = meta.modified() else {
                report.errors += 1;
                continue;
            };
            // Keep anything modified at or after the cutoff (claude `mtime>=e`).
            if mtime >= cutoff {
                continue;
            }
            let path = entry.path();
            let removed = if meta.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            match removed {
                Ok(()) => report.session_files_deleted += 1,
                Err(_) => report.errors += 1,
            }
        }
    }
    report
}

/// Startup entry point: if the sweep is enabled, run it against the resolved
/// config-home and emit `tengu_retention_sweep`. A no-op (and silent) when
/// disabled. Returns the report when it ran, `None` when gated off.
pub fn run_startup_retention_sweep() -> Option<RetentionReport> {
    if !retention_sweep_enabled() {
        return None;
    }
    let (days, used_default) = retention_period_days();
    let report =
        run_retention_sweep_in(&crate::session_memory::config_home_dir(), day_period(days));
    telemetry::emit_retention_sweep(
        false,
        None,
        0, // transcripts_deleted — not ported (session-file sweep only)
        report.session_files_deleted,
        report.errors,
        days,
        used_default,
    );
    Some(report)
}

/// `days` as a [`Duration`] (saturating, so an absurd override can't overflow).
fn day_period(days: u64) -> Duration {
    Duration::from_secs(days.saturating_mul(24 * 60 * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    /// Set an entry's mtime to `age` in the past.
    fn set_old(path: &Path, age: Duration) {
        let when = SystemTime::now() - age;
        let ft = filetime::FileTime::from_system_time(when);
        filetime::set_file_mtime(path, ft).unwrap();
    }

    #[test]
    fn deletes_only_entries_older_than_the_period() {
        let root = tempfile::tempdir().unwrap();
        let todos = root.path().join("todos");
        fs::create_dir_all(&todos).unwrap();
        let old = todos.join("old.json");
        let fresh = todos.join("fresh.json");
        fs::write(&old, b"{}").unwrap();
        fs::write(&fresh, b"{}").unwrap();
        set_old(&old, Duration::from_secs(40 * 86400)); // 40 days old
        // `fresh` keeps its just-now mtime.

        let report = run_retention_sweep_in(root.path(), day_period(30));
        assert_eq!(report.session_files_deleted, 1);
        assert_eq!(report.errors, 0);
        assert!(!old.exists(), "40-day-old file must be swept");
        assert!(fresh.exists(), "fresh file must be kept");
    }

    #[test]
    fn removes_old_directories_recursively() {
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join("logs");
        let old_dir = logs.join("2020-01");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("a.log"), b"x").unwrap();
        set_old(&old_dir, Duration::from_secs(365 * 86400));

        let report = run_retention_sweep_in(root.path(), day_period(30));
        assert_eq!(report.session_files_deleted, 1);
        assert!(!old_dir.exists());
    }

    #[test]
    fn ignores_untracked_dirs_and_missing_subdirs() {
        let root = tempfile::tempdir().unwrap();
        // A directory NOT in the swept set must be untouched even if ancient.
        let other = root.path().join("sessions");
        fs::create_dir_all(&other).unwrap();
        let keep = other.join("important.jsonl");
        fs::write(&keep, b"x").unwrap();
        set_old(&keep, Duration::from_secs(999 * 86400));

        let report = run_retention_sweep_in(root.path(), day_period(30));
        assert_eq!(report.session_files_deleted, 0);
        assert!(keep.exists(), "non-swept directory must never be touched");
    }

    #[test]
    fn enable_flag_defaults_off() {
        // Ensure a clean env for the assertion.
        let prev = std::env::var_os(RETENTION_ENABLE_ENV);
        std::env::remove_var(RETENTION_ENABLE_ENV);
        assert!(!retention_sweep_enabled());
        std::env::set_var(RETENTION_ENABLE_ENV, "1");
        assert!(retention_sweep_enabled());
        std::env::set_var(RETENTION_ENABLE_ENV, "0");
        assert!(!retention_sweep_enabled());
        match prev {
            Some(v) => std::env::set_var(RETENTION_ENABLE_ENV, v),
            None => std::env::remove_var(RETENTION_ENABLE_ENV),
        }
    }

    #[test]
    fn period_defaults_to_30_days() {
        let prev = std::env::var_os(RETENTION_PERIOD_ENV);
        std::env::remove_var(RETENTION_PERIOD_ENV);
        assert_eq!(retention_period_days(), (30, true));
        std::env::set_var(RETENTION_PERIOD_ENV, "7");
        assert_eq!(retention_period_days(), (7, false));
        std::env::set_var(RETENTION_PERIOD_ENV, "0"); // invalid → default
        assert_eq!(retention_period_days(), (30, true));
        match prev {
            Some(v) => std::env::set_var(RETENTION_PERIOD_ENV, v),
            None => std::env::remove_var(RETENTION_PERIOD_ENV),
        }
    }
}
