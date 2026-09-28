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
//! Keys match claude-code's project-config field names so the persisted shape
//! is faithful: `lastCost`, `lastSessionId`, `lastAPIDuration`,
//! `lastTotalInputTokens`, `lastTotalOutputTokens`,
//! `lastTotalCacheCreationInputTokens`, `lastTotalCacheReadInputTokens`, and
//! the per-model `lastModelUsage` breakdown.

use std::path::Path;

/// claude-code project-config key: the last session's cumulative cost (USD).
const LAST_COST: &str = "lastCost";
/// claude-code project-config key: which session `lastCost` belongs to.
const LAST_SESSION_ID: &str = "lastSessionId";
/// Cumulative API wall time (claude-code `lastAPIDuration: yT()`).
const LAST_API_DURATION: &str = "lastAPIDuration";
/// Per-model usage rows (claude-code `lastModelUsage`).
const LAST_MODEL_USAGE: &str = "lastModelUsage";
/// Cumulative input tokens (claude-code `lastTotalInputTokens`).
const LAST_TOTAL_INPUT_TOKENS: &str = "lastTotalInputTokens";
/// Cumulative output tokens (claude-code `lastTotalOutputTokens`).
const LAST_TOTAL_OUTPUT_TOKENS: &str = "lastTotalOutputTokens";
/// Cumulative cache-creation input tokens.
const LAST_TOTAL_CACHE_CREATION_INPUT_TOKENS: &str = "lastTotalCacheCreationInputTokens";
/// Cumulative cache-read input tokens.
const LAST_TOTAL_CACHE_READ_INPUT_TOKENS: &str = "lastTotalCacheReadInputTokens";

/// The `lastModelUsage` value for one model — claude-code writes
/// `{inputTokens, outputTokens, cacheReadInputTokens, cacheCreationInputTokens,
/// costUSD}` per model id.
fn model_usage_entry(row: &platform_api::orchestrator::ModelUsageRow) -> serde_json::Value {
    serde_json::json!({
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheReadInputTokens": row.cache_read_input_tokens,
        "cacheCreationInputTokens": row.cache_creation_input_tokens,
        "costUSD": row.total_nano_usd as f64 / 1e9,
    })
}

/// Persist `total_usd` as this project's `lastCost`, tagged with `session_id`
/// as `lastSessionId`, so a later `--resume <session_id>` can restore it
/// (`saveCurrentSessionCosts`). Best-effort: a write failure is swallowed —
/// cost persistence must never fail an otherwise-clean session exit.
pub fn save_session_cost(
    config_path: &Path,
    cwd: &Path,
    session_id: &str,
    cost: &platform_api::orchestrator::CostSnapshot,
) {
    let key = migrations::global_config::project_path_for_config(cwd);
    let session_id = session_id.to_string();
    let total_usd = cost.total_usd;
    let api_duration_ms = u64::try_from(cost.api_duration.as_millis()).unwrap_or(u64::MAX);
    let (input_tokens, output_tokens) = (cost.input_tokens, cost.output_tokens);
    let (cache_creation, cache_read) = (cost.cache_creation_tokens, cost.cache_read_tokens);
    let mut model_usage = serde_json::Map::new();
    for row in &cost.by_model {
        model_usage.insert(row.model.clone(), model_usage_entry(row));
    }
    let _ = migrations::global_config::save_project_config(config_path, &key, |mut p| {
        p.insert(LAST_COST.to_string(), serde_json::json!(total_usd));
        p.insert(LAST_SESSION_ID.to_string(), serde_json::json!(session_id));
        p.insert(
            LAST_API_DURATION.to_string(),
            serde_json::json!(api_duration_ms),
        );
        p.insert(
            LAST_TOTAL_INPUT_TOKENS.to_string(),
            serde_json::json!(input_tokens),
        );
        p.insert(
            LAST_TOTAL_OUTPUT_TOKENS.to_string(),
            serde_json::json!(output_tokens),
        );
        p.insert(
            LAST_TOTAL_CACHE_CREATION_INPUT_TOKENS.to_string(),
            serde_json::json!(cache_creation),
        );
        p.insert(
            LAST_TOTAL_CACHE_READ_INPUT_TOKENS.to_string(),
            serde_json::json!(cache_read),
        );
        p.insert(
            LAST_MODEL_USAGE.to_string(),
            serde_json::Value::Object(model_usage.clone()),
        );
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

    /// A snapshot carrying one model row, so the persisted shape is exercised
    /// rather than the empty-default path.
    fn snapshot(total_usd: f64) -> platform_api::orchestrator::CostSnapshot {
        platform_api::orchestrator::CostSnapshot {
            total_usd,
            input_tokens: 100,
            output_tokens: 40,
            cache_read_tokens: 25,
            cache_creation_tokens: 10,
            api_duration: std::time::Duration::from_millis(1234),
            by_model: vec![platform_api::orchestrator::ModelUsageRow {
                model: "claude-opus-5".to_string(),
                provider: None,
                total_nano_usd: 17_500_000,
                input_tokens: 100,
                output_tokens: 40,
                cache_read_input_tokens: 25,
                cache_creation_input_tokens: 10,
            }],
            ..Default::default()
        }
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
        save_session_cost(&cfg, cwd, "sess-1", &snapshot(0.0175));
        let proj = project(&cfg, cwd);
        assert_eq!(
            proj.get(LAST_COST).and_then(serde_json::Value::as_f64),
            Some(0.0175)
        );
        assert_eq!(
            proj.get(LAST_SESSION_ID)
                .and_then(serde_json::Value::as_str),
            Some("sess-1")
        );
    }

    #[test]
    fn keeps_projects_apart() {
        let (_dir, cfg) = tmp_config();
        save_session_cost(&cfg, Path::new("/proj/alpha"), "sess-1", &snapshot(0.0175));
        // Same session id, different project key: no cross-project bleed.
        let beta = project(&cfg, Path::new("/proj/beta"));
        assert!(beta.get(LAST_COST).is_none());
    }

    #[test]
    fn later_save_overwrites_the_projects_last_cost() {
        let (_dir, cfg) = tmp_config();
        let cwd = Path::new("/proj/alpha");
        save_session_cost(&cfg, cwd, "sess-1", &snapshot(0.01));
        save_session_cost(&cfg, cwd, "sess-2", &snapshot(0.05));
        // One slot per project, matching claude-code's single `lastSessionId`.
        let proj = project(&cfg, cwd);
        assert_eq!(
            proj.get(LAST_COST).and_then(serde_json::Value::as_f64),
            Some(0.05)
        );
        assert_eq!(
            proj.get(LAST_SESSION_ID)
                .and_then(serde_json::Value::as_str),
            Some("sess-2")
        );
    }

    /// CLI-7 — claude-code persists the whole usage picture, not just the money
    /// total: `lastAPIDuration`, the four cumulative token counters, and the
    /// per-model `lastModelUsage` breakdown. Restoring only `lastCost` left a
    /// resumed session unable to rebuild "Usage by model" or the API duration.
    #[test]
    fn every_usage_key_claude_code_writes_is_persisted() {
        let (_dir, cfg) = tmp_config();
        let cwd = Path::new("/proj/alpha");
        save_session_cost(&cfg, cwd, "sess-1", &snapshot(0.0175));
        let proj = project(&cfg, cwd);

        assert_eq!(
            proj.get(LAST_API_DURATION)
                .and_then(serde_json::Value::as_u64),
            Some(1234)
        );
        assert_eq!(
            proj.get(LAST_TOTAL_INPUT_TOKENS)
                .and_then(serde_json::Value::as_u64),
            Some(100)
        );
        assert_eq!(
            proj.get(LAST_TOTAL_OUTPUT_TOKENS)
                .and_then(serde_json::Value::as_u64),
            Some(40)
        );
        assert_eq!(
            proj.get(LAST_TOTAL_CACHE_READ_INPUT_TOKENS)
                .and_then(serde_json::Value::as_u64),
            Some(25)
        );
        assert_eq!(
            proj.get(LAST_TOTAL_CACHE_CREATION_INPUT_TOKENS)
                .and_then(serde_json::Value::as_u64),
            Some(10)
        );
    }

    /// `lastModelUsage` is keyed by model id and carries the oracle's five
    /// fields, with cost in USD (the port stores nano-USD internally).
    #[test]
    fn the_per_model_breakdown_is_keyed_by_model_with_cost_in_usd() {
        let (_dir, cfg) = tmp_config();
        let cwd = Path::new("/proj/alpha");
        save_session_cost(&cfg, cwd, "sess-1", &snapshot(0.0175));
        let proj = project(&cfg, cwd);

        let usage = proj
            .get(LAST_MODEL_USAGE)
            .and_then(serde_json::Value::as_object)
            .expect("lastModelUsage is an object keyed by model");
        let row = usage
            .get("claude-opus-5")
            .and_then(serde_json::Value::as_object)
            .expect("the model row");
        assert_eq!(
            row.get("inputTokens").and_then(serde_json::Value::as_u64),
            Some(100)
        );
        assert_eq!(
            row.get("outputTokens").and_then(serde_json::Value::as_u64),
            Some(40)
        );
        assert_eq!(
            row.get("cacheReadInputTokens")
                .and_then(serde_json::Value::as_u64),
            Some(25)
        );
        assert_eq!(
            row.get("cacheCreationInputTokens")
                .and_then(serde_json::Value::as_u64),
            Some(10)
        );
        // 17_500_000 nano-USD = 0.0175 USD.
        let cost = row
            .get("costUSD")
            .and_then(serde_json::Value::as_f64)
            .expect("costUSD");
        assert!(
            (cost - 0.0175).abs() < 1e-9,
            "costUSD must be USD, got {cost}"
        );
    }
}
