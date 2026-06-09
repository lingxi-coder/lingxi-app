//! `migrateSonnet1mToSonnet45.ts` — pin users who saved `sonnet[1m]` to the
//! explicit `sonnet-4-5-20250929[1m]` (the bare alias now resolves to 4.6).
//! Reads userSettings specifically (NOT merged) so a project-scoped pin isn't
//! promoted to the global default. Run-once via the
//! `sonnet1m45MigrationComplete` global-config flag (set even when the model
//! didn't match — TS parity).
//!
//! NOT ported: the TS in-memory `MainLoopModelOverride` sub-step
//! (`migrateSonnet1mToSonnet45.ts:39-42`) — no equivalent pre-boot in-memory
//! model state exists in the Rust CLI.

use crate::context::{js_truthy, MigrationEnv};
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use serde_json::{json, Value};

/// Run the migration (errors swallowed with a warn, never abort startup).
// async for the uniform migration-runner API; this one emits no telemetry,
// so it has no await point.
#[allow(clippy::unused_async)]
pub async fn run(env: &MigrationEnv) {
    let cfg = match global_config::read_map(&env.global_config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "migrate_sonnet1m_to_sonnet45: config read failed");
            return;
        }
    };
    if cfg.get("sonnet1m45MigrationComplete").is_some_and(js_truthy) {
        return;
    }

    let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
    let model = read_settings_map(&sp)
        .ok()
        .and_then(|m| m.get("model").and_then(Value::as_str).map(String::from));
    if model.as_deref() == Some("sonnet[1m]") {
        if let Err(e) = update_settings(
            &sp,
            vec![("model".into(), Some(json!("sonnet-4-5-20250929[1m]")))],
        ) {
            // TS throws here, so the completion flag below is NOT set and the
            // migration retries next startup — mirror that by returning.
            tracing::warn!(error = %e, "migrate_sonnet1m_to_sonnet45: settings write failed");
            return;
        }
    }

    if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
        m.insert("sonnet1m45MigrationComplete".into(), Value::Bool(true));
        m
    }) {
        tracing::warn!(error = %e, "migrate_sonnet1m_to_sonnet45: flag write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::{read_settings_map, settings_path, SettingsSource};
    use crate::test_support::temp_config;
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
    async fn rewrites_sonnet1m_and_sets_flag() {
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "sonnet[1m]", "keep": true}"#).unwrap();
        run(&test_env(&t)).await;
        let s = read_settings_map(&sp).unwrap();
        assert_eq!(s["model"], json!("sonnet-4-5-20250929[1m]"));
        assert_eq!(s["keep"], json!(true));
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["sonnet1m45MigrationComplete"], json!(true));
    }

    #[tokio::test]
    async fn other_model_untouched_but_flag_still_set() {
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "opus"}"#).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("opus"));
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["sonnet1m45MigrationComplete"], json!(true));
    }

    #[tokio::test]
    async fn completion_flag_short_circuits() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"sonnet1m45MigrationComplete": true}"#).unwrap();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "sonnet[1m]"}"#).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("sonnet[1m]"));
    }
}
