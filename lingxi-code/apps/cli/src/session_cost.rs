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

/// Restore this project's persisted `lastCost` (USD) IF it was saved for
/// `session_id` — i.e. the resumed session was the last one saved for this
/// project (`getStoredSessionCosts`' `lastSessionId !== sessionId` gate).
/// Returns `None` otherwise: a fresh or mismatched session must start at zero.
#[must_use]
pub fn restore_session_cost_usd(config_path: &Path, cwd: &Path, session_id: &str) -> Option<f64> {
    let key = migrations::global_config::project_path_for_config(cwd);
    let proj = migrations::global_config::get_project_config(config_path, &key).ok()?;
    let last_session = proj
        .get(LAST_SESSION_ID)
        .and_then(serde_json::Value::as_str)?;
    if last_session != session_id {
        return None;
    }
    proj.get(LAST_COST).and_then(serde_json::Value::as_f64)
}

/// USD → nano-USD for seeding
/// [`orchestrator::ConversationOrchestrator::restore_session_cost`]. A
/// non-positive value clamps to `0` (the tracker's zero baseline).
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "session cost is bounded well within u64 after the >0 guard; sub-nano rounding is immaterial"
)]
pub fn usd_to_nano(usd: f64) -> u64 {
    if usd > 0.0 {
        (usd * 1_000_000_000.0).round() as u64
    } else {
        0
    }
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

    #[test]
    fn round_trips_cost_when_session_matches() {
        let (_dir, cfg) = tmp_config();
        let cwd = Path::new("/proj/alpha");
        save_session_cost(&cfg, cwd, "sess-1", 0.0175);
        let restored = restore_session_cost_usd(&cfg, cwd, "sess-1");
        assert_eq!(restored, Some(0.0175));
    }

    #[test]
    fn does_not_restore_for_a_different_session() {
        let (_dir, cfg) = tmp_config();
        let cwd = Path::new("/proj/alpha");
        save_session_cost(&cfg, cwd, "sess-1", 0.0175);
        // A fresh (different-id) launch must start at zero, not inherit sess-1.
        assert_eq!(restore_session_cost_usd(&cfg, cwd, "sess-2"), None);
    }

    #[test]
    fn does_not_restore_for_a_different_project() {
        let (_dir, cfg) = tmp_config();
        save_session_cost(&cfg, Path::new("/proj/alpha"), "sess-1", 0.0175);
        // Same session id, different project key ⇒ no cross-project bleed.
        assert_eq!(
            restore_session_cost_usd(&cfg, Path::new("/proj/beta"), "sess-1"),
            None
        );
    }

    #[test]
    fn missing_config_restores_nothing() {
        let (_dir, cfg) = tmp_config(); // file never written
        assert_eq!(
            restore_session_cost_usd(&cfg, Path::new("/proj/alpha"), "sess-1"),
            None
        );
    }


    #[test]
    fn later_save_overwrites_the_projects_last_cost() {
        let (_dir, cfg) = tmp_config();
        let cwd = Path::new("/proj/alpha");
        save_session_cost(&cfg, cwd, "sess-1", 0.01);
        save_session_cost(&cfg, cwd, "sess-2", 0.05);
        // Only the most-recent session's cost is restorable (matches
        // claude-code's single `lastSessionId` slot).
        assert_eq!(restore_session_cost_usd(&cfg, cwd, "sess-1"), None);
        assert_eq!(restore_session_cost_usd(&cfg, cwd, "sess-2"), Some(0.05));
    }

    #[test]
    fn usd_to_nano_rounds_and_clamps() {
        assert_eq!(usd_to_nano(0.0175), 17_500_000);
        assert_eq!(usd_to_nano(0.0), 0);
        assert_eq!(usd_to_nano(-1.0), 0);
    }
}
