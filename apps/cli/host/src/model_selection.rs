//! Durable interactive model picks. SDK overrides remain session-scoped.
use platform_api::{HandleError, OrchestratorHandle};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(crate) fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| memory::lingxi_md::user_config_dir(&home).join("settings.json"))
}

pub(crate) async fn apply(
    orchestrator: &dyn OrchestratorHandle,
    model: &str,
    profile: Option<&str>,
    settings_path: Option<&Path>,
) -> Result<(), HandleError> {
    orchestrator
        .switch_model_with_source(model, profile, "picker")
        .await?;
    if let Some(path) = settings_path {
        if let Err(error) = remember_at(path, model, profile) {
            // The live switch has succeeded; a disk failure must not make the
            // UI claim the engine rejected the selection.
            tracing::warn!(%error, "could not persist the selected model");
        }
    }
    Ok(())
}

fn remember_at(path: &Path, model: &str, profile: Option<&str>) -> Result<(), String> {
    let qualified = profile.map_or_else(|| model.to_string(), |p| format!("{p}/{model}"));
    let mut updates = vec![("model".into(), Some(Value::String(qualified)))];
    if let Some(profile) = profile {
        let config = migrations::settings_update::read_settings_map(path)?;
        let mut recents = config
            .get("recentModels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        recents.retain(|entry| entry.get("model").and_then(Value::as_str) != Some(model));
        recents.insert(0, json!({"provider": profile, "model": model}));
        recents.truncate(tui_core::recent_models::MAX_RECENT);
        updates.push(("recentModels".into(), Some(Value::Array(recents))));
    }
    // Reuse the settings writer's path lock and atomic replacement, preserving
    // concurrent updates of unrelated settings rather than rewriting a snapshot.
    migrations::settings_update::update_settings(path, updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_survive_reopen_without_losing_settings_or_provider() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","permissions":{"defaultMode":"plan"}}"#,
        )
        .unwrap();
        remember_at(&path, "first", Some("provider-a")).unwrap();
        remember_at(&path, "latest", Some("provider-b")).unwrap();
        let saved = migrations::global_config::read_map(&path).unwrap();
        assert_eq!(saved["model"], "provider-b/latest");
        assert_eq!(
            saved["recentModels"][0],
            json!({"provider":"provider-b","model":"latest"})
        );
        assert_eq!(saved["theme"], "dark");
        assert_eq!(saved["permissions"]["defaultMode"], "plan");
        // This is the same SettingsJson shape the CLI startup loader reads.
        let settings: lingxi_core::settings::SettingsJson =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(settings.model.as_deref(), Some("provider-b/latest"));
    }

    #[test]
    fn invalid_settings_are_not_destroyed_by_a_model_pick() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        std::fs::write(&path, "invalid json").unwrap();
        assert!(remember_at(&path, "latest", None).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid json");
    }
}
