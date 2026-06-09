//! `migrateLegacyOpusToCurrent.ts` — move first-party users off explicit
//! Opus 4.0/4.1 strings to the `opus` alias; stamp
//! `legacyOpusMigrationTimestamp` for the (unported) REPL one-time notice.
//! Idempotent by construction: once rewritten, the model no longer matches.

use crate::context::{env_var_truthy, MigrationEnv};
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use serde_json::{json, Value};
use telemetry::sink::AnalyticsValue;

/// The four explicit legacy strings (`migrateLegacyOpusToCurrent.ts:41-46`).
const LEGACY_MODELS: [&str; 4] = [
    "claude-opus-4-20250514",
    "claude-opus-4-1-20250805",
    "claude-opus-4-0",
    "claude-opus-4-1",
];

/// Run the migration.
pub async fn run(env: &MigrationEnv) {
    if !env.ctx.first_party {
        return;
    }
    // isLegacyModelRemapEnabled (`model.ts:552-554`) = NOT env-truthy opt-out.
    if env_var_truthy("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP") {
        return;
    }

    let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
    let Some(model) = read_settings_map(&sp)
        .ok()
        .and_then(|m| m.get("model").and_then(Value::as_str).map(String::from))
    else {
        return;
    };
    if !LEGACY_MODELS.contains(&model.as_str()) {
        return;
    }

    if let Err(e) = update_settings(&sp, vec![("model".into(), Some(json!("opus")))]) {
        // TS `updateSettingsForSource` never throws — it returns `{error}`
        // (settings.ts:416-523) and the migration discards it (TS:48), then
        // still stamps the timestamp and emits the event. Warn and continue.
        tracing::warn!(error = %e, "migrate_legacy_opus: settings write failed (ignored, TS parity)");
    }
    if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
        m.insert(
            "legacyOpusMigrationTimestamp".into(),
            json!(MigrationEnv::now_ms()),
        );
        m
    }) {
        // TS throws here, before logEvent — mirror that by skipping the emit.
        tracing::warn!(error = %e, "migrate_legacy_opus: timestamp write failed");
        return;
    }
    env.emit(
        telemetry::tengu::migration::LEGACY_OPUS_MIGRATION,
        std::collections::HashMap::from([(
            "from_model".to_string(),
            AnalyticsValue::String(model),
        )]),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::{read_settings_map, settings_path, SettingsSource};
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

    // env_lock is a std Mutex held across `.await` on purpose: it serializes
    // whole test bodies against parallel test threads, and each #[tokio::test]
    // runs its own current-thread runtime, so no executor task can deadlock on it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn rewrites_each_legacy_string_and_stamps_timestamp() {
        let _g = env_lock(); // reads CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        for legacy in [
            "claude-opus-4-20250514",
            "claude-opus-4-1-20250805",
            "claude-opus-4-0",
            "claude-opus-4-1",
        ] {
            let t = temp_config();
            let sp = settings_path(SettingsSource::User, &t.home, &t.project);
            std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
            std::fs::write(&sp, format!(r#"{{"model": "{legacy}"}}"#)).unwrap();
            run(&test_env(&t)).await;
            assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("opus"));
            let m = crate::global_config::read_map(&t.global).unwrap();
            assert!(m["legacyOpusMigrationTimestamp"].is_i64());
        }
    }

    /// Fix 2 continue branch: a failing settings WRITE must not block the
    /// timestamp stamp + event (TS `updateSettingsForSource` returns an
    /// ignored `{error}`, settings.ts:416-523; TS:48-56 proceed regardless).
    // See rewrites_each_legacy_string_and_stamps_timestamp for the lock rationale.
    #[allow(clippy::await_holding_lock)]
    #[cfg(unix)]
    #[tokio::test]
    async fn settings_write_failure_still_stamps_timestamp() {
        use std::os::unix::fs::PermissionsExt;
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "claude-opus-4-1"}"#).unwrap();
        std::fs::set_permissions(&sp, std::fs::Permissions::from_mode(0o444)).unwrap();
        // Skip when perms don't bite (e.g. running as root).
        if std::fs::OpenOptions::new().append(true).open(&sp).is_ok() {
            return;
        }
        run(&test_env(&t)).await;
        std::fs::set_permissions(&sp, std::fs::Permissions::from_mode(0o644)).unwrap();
        // write failed → model unchanged…
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("claude-opus-4-1"));
        // …but the timestamp is STILL stamped (and the emit ran; bus is None).
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m["legacyOpusMigrationTimestamp"].is_i64());
    }

    // See rewrites_each_legacy_string_and_stamps_timestamp for the rationale.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn non_first_party_or_optout_or_other_model_noop() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "claude-opus-4-0"}"#).unwrap();

        // not first-party
        let mut env = test_env(&t);
        env.ctx.first_party = false;
        run(&env).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("claude-opus-4-0"));

        // env opt-out
        std::env::set_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP", "1");
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("claude-opus-4-0"));
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");

        // non-legacy model
        std::fs::write(&sp, r#"{"model": "sonnet"}"#).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("sonnet"));
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("legacyOpusMigrationTimestamp").is_none());
    }
}
