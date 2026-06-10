//! `runMigrations` (`main.tsx:323-353`) — version-guarded startup migration
//! set. Bump [`CURRENT_MIGRATION_VERSION`] when adding a sync migration so
//! existing users re-run the set.

use crate::context::MigrationEnv;
use crate::global_config;
use serde_json::{json, Value};

/// `CURRENT_MIGRATION_VERSION` (`main.tsx:325`).
pub const CURRENT_MIGRATION_VERSION: u64 = 11;

/// Run the sync migration set if `migrationVersion != 11`, then bump.
/// Mirrors the TS guard exactly (`!==`, so a downgrade re-runs too).
///
/// SAFETY CONTRACT: a broken/unreadable `~/.claude.json` skips the whole run
/// (no writes, no version bump — stricter than TS's defaults-fallback,
/// documented divergence). Per-migration failure handling DELIBERATELY
/// DIVERGES from TS: `runMigrations` has NO catch, so a `saveGlobalConfig`
/// throw inside one migration aborts the remaining migrations AND the
/// version bump — the set retries next startup, but the TS CLI startup
/// crashes. This port instead warns-and-continues per migration, so startup
/// never crashes. In the DOMINANT failure mode (unwritable global config)
/// the final version-bump save below fails for the same reason, so no bump
/// happens and the set naturally retries next startup like TS; only a
/// transient mid-run failure would be bumped past — accepted divergence,
/// since every migration is individually idempotent or run-once-flagged.
/// The async changelog migration is NOT part of this fn — the caller spawns
/// [`crate::changelog::migrate_changelog_from_config`] fire-and-forget.
pub async fn run_migrations(env: &MigrationEnv) {
    let cfg = match global_config::read_map(&env.global_config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "run_migrations: global config unreadable; skipping this startup");
            return;
        }
    };
    let version = cfg.get("migrationVersion").and_then(Value::as_u64);
    if version == Some(CURRENT_MIGRATION_VERSION) {
        return;
    }

    // TS execution order (main.tsx:328-336). migrateFennecToOpus (ant-only
    // dead code) and resetAutoModeOptInForDefaultOffer (TRANSCRIPT_CLASSIFIER
    // gate) are intentionally absent — see lib.rs module docs.
    crate::migrate_auto_updates::run(env).await;
    crate::migrate_bypass_permissions::run(env).await;
    crate::migrate_mcp_servers::run(env).await;
    crate::migrate_reset_pro_to_opus::run(env).await;
    crate::migrate_sonnet1m_to_sonnet45::run(env).await;
    crate::migrate_legacy_opus::run(env).await;
    crate::migrate_sonnet45_to_46::run(env).await;
    crate::migrate_opus_to_opus1m::run(env).await;
    crate::migrate_repl_bridge::run(env).await;

    if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
        if m.get("migrationVersion").and_then(Value::as_u64) == Some(CURRENT_MIGRATION_VERSION) {
            return m; // TS: prev.migrationVersion === CURRENT ? prev : …
        }
        m.insert("migrationVersion".into(), json!(CURRENT_MIGRATION_VERSION));
        m
    }) {
        tracing::warn!(error = %e, "run_migrations: version bump failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{env_lock, temp_config};
    use serde_json::json;

    fn test_env(t: &crate::test_support::TempConfig) -> crate::context::MigrationEnv {
        crate::context::MigrationEnv {
            global_config_path: t.global.clone(),
            claude_config_home: t.home.clone(),
            project_dir: t.project.clone(),
            ctx: crate::context::MigrationContext { first_party: true, subscription_type: None },
            bus: None,
        }
    }

    #[tokio::test]
    async fn at_version_11_is_a_pure_readonly_noop() {
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"migrationVersion": 11, "replBridgeEnabled": true}"#,
        )
        .unwrap();
        let before = std::fs::read_to_string(&t.global).unwrap();
        run_migrations(&test_env(&t)).await;
        // version guard: nothing runs, nothing written (replBridge NOT renamed)
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), before);
    }

    // env_lock is a std Mutex held across `.await` on purpose: it serializes
    // whole test bodies against parallel test threads, and each #[tokio::test]
    // runs its own current-thread runtime, so no executor task can deadlock on it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn below_version_runs_set_and_bumps_to_11() {
        let _g = env_lock(); // legacy-opus migration reads env opt-out
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        std::fs::write(&t.global, r#"{"replBridgeEnabled": true}"#).unwrap();
        run_migrations(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["migrationVersion"], json!(11));
        assert_eq!(m["remoteControlAtStartup"], json!(true));
        assert!(m.get("replBridgeEnabled").is_none());
        // run-once flags from the always-mark migrations
        assert_eq!(m["sonnet1m45MigrationComplete"], json!(true));
        assert_eq!(m["opusProMigrationComplete"], json!(true));
    }

    // See below_version_runs_set_and_bumps_to_11 for the lock rationale.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn second_run_is_noop_after_bump() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        run_migrations(&test_env(&t)).await;
        let after_first = std::fs::read_to_string(&t.global).unwrap();
        run_migrations(&test_env(&t)).await;
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), after_first);
    }

    // See below_version_runs_set_and_bumps_to_11 for the lock rationale.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn version_above_11_reruns_like_ts() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"migrationVersion": 12, "replBridgeEnabled": true}"#,
        )
        .unwrap();
        run_migrations(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        // TS guard is `!== CURRENT`, so 12 re-runs and lands on 11
        assert_eq!(m["migrationVersion"], json!(11));
        assert!(m.get("replBridgeEnabled").is_none());
    }

    /// Emission contract: at version 11 the guard short-circuits before any
    /// migration runs — zero events, even with a live bus.
    #[tokio::test]
    async fn at_version_11_emits_nothing() {
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"migrationVersion": 11, "replBridgeEnabled": true}"#,
        )
        .unwrap();
        let (bus, events) = crate::test_support::capture_bus().await;
        let mut env = test_env(&t);
        env.bus = Some(bus);
        run_migrations(&env).await;
        assert!(events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn broken_global_config_skips_run_untouched() {
        let t = temp_config();
        std::fs::write(&t.global, "{ broken").unwrap();
        run_migrations(&test_env(&t)).await;
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), "{ broken");
    }
}
