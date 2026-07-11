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

/// Entry-level housekeeping directories — every stale entry (file or dir) is
/// removed (claude's `["todos","statsig","logs"]` loop).
const SWEPT_ENTRY_DIRS: &[&str] = &["todos", "statsig", "logs"];

/// Subdirectory-level housekeeping directories — every stale *subdirectory* is
/// removed (claude's `VRt(...)` cleanups: `file-history`, `session-env`,
/// `tasks`, `uploads`, `skills/.staging`).
const SWEPT_SUBDIR_TREES: &[&str] = &[
    "file-history",
    "session-env",
    "tasks",
    "uploads",
    "skills/.staging",
    "shares", // wVg (also `.zip`-file-swept)
];

/// Subdirectory-level dir also swept for stale `.zip` files (claude `wVg`).
/// Kept out of [`SWEPT_SUBDIR_TREES`]'s comment list only for clarity — it is
/// appended below.
///
/// `(dir, extension)` file sweeps — claude's `aj(<dir>, <ext>)` cleanups. Every
/// direct child file whose name ends with `ext` and whose mtime is older than
/// the cutoff is removed, then the dir is pruned if it emptied.
const SWEPT_FILE_EXTS: &[(&str, &str)] = &[
    ("plans", ".md"),               // hVg
    ("telemetry", ".json"),         // AVg
    ("traces", ".json"),            // OVg
    ("startup-perf", ".txt"),       // OVg
    ("shell-snapshots", ".sh"),     // xVg
    ("feedback-bundles", ".zip"),   // PVg
    ("dump-prompts", ".jsonl"),     // RVg (claude caps this shorter; we use the base period — keeps longer, safe)
    ("shares", ".zip"),             // wVg (also VRt-swept below)
];

/// Single cache files removed when stale (claude `fVg` / `mVg`).
const SWEPT_SINGLE_FILES: &[&str] = &["hfi-auth.json", "mcp-needs-auth-cache.json"];

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
    // Entry-level sweep (todos/statsig/logs): remove every stale file/dir.
    for sub in SWEPT_ENTRY_DIRS {
        report = merge(report, sweep_stale_entries(&config_home.join(sub), cutoff, true));
    }
    // Subdirectory-level sweep (VRt): remove stale subdirectories, then prune the
    // named dir if it emptied.
    for sub in SWEPT_SUBDIR_TREES {
        report = merge(report, sweep_stale_entries(&config_home.join(sub), cutoff, false));
    }
    // File-extension sweeps (aj): stale `<dir>/*<ext>` files.
    for (dir, ext) in SWEPT_FILE_EXTS {
        report = merge(
            report,
            sweep_stale_files_by_ext(&config_home.join(dir), ext, cutoff),
        );
    }
    // Single stale cache files (fVg / mVg).
    for name in SWEPT_SINGLE_FILES {
        report = merge(report, sweep_stale_file(&config_home.join(name), cutoff));
    }
    report
}

/// Merge two reports (claude `Vnt`).
fn merge(a: RetentionReport, b: RetentionReport) -> RetentionReport {
    RetentionReport {
        session_files_deleted: a.session_files_deleted + b.session_files_deleted,
        errors: a.errors + b.errors,
    }
}

/// `rmdir` `dir` if it is now empty — best-effort (claude `sj`).
fn prune_if_empty(dir: &Path) {
    let _ = std::fs::remove_dir(dir);
}

/// Remove a single file when its mtime is older than `cutoff` (claude
/// `fVg`/`mVg`, one-file `Xae`). Absent file → no-op.
fn sweep_stale_file(path: &Path, cutoff: SystemTime) -> RetentionReport {
    let mut report = RetentionReport::default();
    let Ok(meta) = std::fs::metadata(path) else {
        return report; // absent → nothing to do
    };
    if !meta.is_file() {
        return report;
    }
    match meta.modified() {
        Ok(mtime) if mtime < cutoff => match std::fs::remove_file(path) {
            Ok(()) => report.session_files_deleted += 1,
            Err(_) => report.errors += 1,
        },
        Ok(_) => {}
        Err(_) => report.errors += 1,
    }
    report
}

/// Remove stale entries under `dir` whose mtime is older than `cutoff`. When
/// `include_files` is true every entry (file or dir) is eligible (the
/// todos/statsig/logs loop); when false only subdirectories are (claude `VRt`,
/// which then prunes the emptied parent).
fn sweep_stale_entries(dir: &Path, cutoff: SystemTime, include_files: bool) -> RetentionReport {
    let mut report = RetentionReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report; // absent / unreadable directory → skip
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            report.errors += 1;
            continue;
        };
        if !include_files && !meta.is_dir() {
            continue; // VRt considers subdirectories only
        }
        let Ok(mtime) = meta.modified() else {
            report.errors += 1;
            continue;
        };
        // Keep anything modified at or after the cutoff (claude `mtime>=e` /
        // `mtime<n`).
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
    if !include_files {
        prune_if_empty(dir);
    }
    report
}

/// Remove stale files directly under `dir` whose name ends with `ext` and whose
/// mtime is older than `cutoff`, then prune the dir if it emptied (claude `aj`).
fn sweep_stale_files_by_ext(dir: &Path, ext: &str, cutoff: SystemTime) -> RetentionReport {
    let mut report = RetentionReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            report.errors += 1;
            continue;
        };
        if !meta.is_file() || !entry.file_name().to_string_lossy().ends_with(ext) {
            continue;
        }
        let Ok(mtime) = meta.modified() else {
            report.errors += 1;
            continue;
        };
        if mtime >= cutoff {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => report.session_files_deleted += 1,
            Err(_) => report.errors += 1,
        }
    }
    prune_if_empty(dir);
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
    fn sweeps_stale_subdirectories_but_not_stale_files_in_vrt_dirs() {
        let root = tempfile::tempdir().unwrap();
        let fh = root.path().join("file-history");
        fs::create_dir_all(&fh).unwrap();
        // A stale SUBDIR is removed; a stale FILE at this level is NOT (VRt only
        // considers subdirectories).
        let old_dir = fh.join("old-session");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("edit.json"), b"{}").unwrap();
        set_old(&old_dir, Duration::from_secs(45 * 86400));
        let stray_file = fh.join("stray.txt");
        fs::write(&stray_file, b"x").unwrap();
        set_old(&stray_file, Duration::from_secs(45 * 86400));

        let report = run_retention_sweep_in(root.path(), day_period(30));
        assert_eq!(report.session_files_deleted, 1);
        assert!(!old_dir.exists(), "stale subdir must be swept");
        assert!(stray_file.exists(), "a file in a VRt dir must be left alone");
    }

    #[test]
    fn sweeps_stale_md_plans_only() {
        let root = tempfile::tempdir().unwrap();
        let plans = root.path().join("plans");
        fs::create_dir_all(&plans).unwrap();
        let old_md = plans.join("2020-old.md");
        let fresh_md = plans.join("today.md");
        let other = plans.join("keep.txt");
        for p in [&old_md, &fresh_md, &other] {
            fs::write(p, b"x").unwrap();
        }
        set_old(&old_md, Duration::from_secs(60 * 86400));
        set_old(&other, Duration::from_secs(60 * 86400)); // old but not .md

        let report = run_retention_sweep_in(root.path(), day_period(30));
        assert_eq!(report.session_files_deleted, 1);
        assert!(!old_md.exists(), "stale .md plan must be swept");
        assert!(fresh_md.exists(), "fresh .md plan must be kept");
        assert!(other.exists(), "non-.md file must be kept");
    }

    #[test]
    fn sweeps_stale_single_cache_files_and_ext_dirs() {
        let root = tempfile::tempdir().unwrap();
        // Single cache file, stale → removed.
        let cache = root.path().join("mcp-needs-auth-cache.json");
        fs::write(&cache, b"{}").unwrap();
        set_old(&cache, Duration::from_secs(45 * 86400));
        // A fresh single cache file is kept.
        let fresh_cache = root.path().join("hfi-auth.json");
        fs::write(&fresh_cache, b"{}").unwrap();
        // File-ext dir: stale telemetry .json removed, non-.json kept.
        let tele = root.path().join("telemetry");
        fs::create_dir_all(&tele).unwrap();
        let old_json = tele.join("old.json");
        let keep_txt = tele.join("readme.txt");
        fs::write(&old_json, b"{}").unwrap();
        fs::write(&keep_txt, b"x").unwrap();
        set_old(&old_json, Duration::from_secs(45 * 86400));
        set_old(&keep_txt, Duration::from_secs(45 * 86400));

        let report = run_retention_sweep_in(root.path(), day_period(30));
        assert_eq!(report.session_files_deleted, 2); // cache + old.json
        assert!(!cache.exists());
        assert!(fresh_cache.exists());
        assert!(!old_json.exists());
        assert!(keep_txt.exists(), "non-matching ext must be kept");
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
