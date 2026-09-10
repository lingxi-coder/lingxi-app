//! Session-cost persistence across `--resume` — the port of claude-code
//! `saveCurrentSessionCosts` / `restoreCostStateForSession` (`cost-tracker.ts`).
//!
//! The interactive cost tracker is per-process and starts at zero, so a resumed
//! session would show `$0.0000` in the footer until its first new turn (the
//! reported "session cost resets to 0.000 after resume" bug). claude-code
//! avoids this by writing the session's total to the PROJECT config on exit
//! (`lastCost` + `lastSessionId`) and restoring it on resume, gated on the
//! resumed id matching the last-saved one. This module ports that persist /
//! restore round trip onto the port's `~/.lingxi.json` project-config substrate
//! ([`migrations::global_config`]).
//!
//! Only the write half lives here. The durable session ledger owns a resumed
//! session's total, so the read half is `capture_legacy_opening_balance` in
//! `engine-desktop`, which imports this figure into the WAL exactly once per
//! session identity. Reading it here as well would add the prior total on top
//! of a projection that already contains it.
//!
//! Keys match claude-code's project-config field names (`lastCost`,
//! `lastSessionId`) so the persisted shape is faithful. Only the money total is
//! carried today — the per-model token breakdown claude-code also stores
//! (`lastModelUsage`, `lastAPIDuration`, …) is not yet persisted; the footer /
//! status-line cost the user sees is derived from the total, so restoring it is
//! sufficient to fix the reported bug. (Documented parity follow-up.)

use std::path::Path;

/// claude-code project-config key: the last session's cumulative cost (USD).
const LAST_COST: &str = "lastCost";
/// claude-code project-config key: which session `lastCost` belongs to.
const LAST_SESSION_ID: &str = "lastSessionId";

/// Persist `total_usd` as this project's `lastCost`, tagged with `session_id`
/// as `lastSessionId`, so a later `--resume <session_id>` can restore it
/// (`saveCurrentSessionCosts`). Best-effort: a write failure is swallowed —
/// cost persistence must never fail an otherwise-clean session exit.
pub fn save_session_cost(config_path: &Path, cwd: &Path, session_id: &str, total_usd: f64) {
    let key = migrations::global_config::project_path_for_config(cwd);
    let session_id = session_id.to_string();
    let _ = migrations::global_config::save_project_config(config_path, &key, |mut p| {
        p.insert(LAST_COST.to_string(), serde_json::json!(total_usd));
        p.insert(LAST_SESSION_ID.to_string(), serde_json::json!(session_id));
        p
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_config() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lingxi.json");
        (dir, path)
    }

    fn project(config: &Path, cwd: &Path) -> serde_json::Map<String, serde_json::Value> {
        let key = migrations::global_config::project_path_for_config(cwd);
        migrations::global_config::get_project_config(config, &key).unwrap()
    }

    /// The reader is `capture_legacy_opening_balance` in another crate, keyed
    /// on these exact two names and types. Assert the shape, not a round trip
    /// through a reader that no longer lives here.
    #[test]
    fn writes_the_two_keys_the_ledger_import_reads() {
        let (_dir, cfg) = tmp_config();
        let cwd = Path::new("/proj/alpha");
        save_session_cost(&cfg, cwd, "sess-1", 0.0175);
        let proj = project(&cfg, cwd);
        assert_eq!(proj.get(LAST_COST).and_then(serde_json::Value::as_f64), Some(0.0175));
        assert_eq!(
            proj.get(LAST_SESSION_ID).and_then(serde_json::Value::as_str),
            Some("sess-1")
        );
    }

    #[test]
    fn keeps_projects_apart() {
        let (_dir, cfg) = tmp_config();
        save_session_cost(&cfg, Path::new("/proj/alpha"), "sess-1", 0.0175);
        // Same session id, different project key: no cross-project bleed.
        let beta = project(&cfg, Path::new("/proj/beta"));
        assert!(beta.get(LAST_COST).is_none());
    }

    #[test]
    fn later_save_overwrites_the_projects_last_cost() {
        let (_dir, cfg) = tmp_config();
        let cwd = Path::new("/proj/alpha");
        save_session_cost(&cfg, cwd, "sess-1", 0.01);
        save_session_cost(&cfg, cwd, "sess-2", 0.05);
        // One slot per project, matching claude-code's single `lastSessionId`.
        let proj = project(&cfg, cwd);
        assert_eq!(proj.get(LAST_COST).and_then(serde_json::Value::as_f64), Some(0.05));
        assert_eq!(
            proj.get(LAST_SESSION_ID).and_then(serde_json::Value::as_str),
            Some("sess-2")
        );
    }
}
