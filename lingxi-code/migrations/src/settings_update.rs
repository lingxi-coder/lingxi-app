//! `updateSettingsForSource` / `getSettingsForSource` port
//! (`utils/settings/settings.ts` L416/L459 semantics) for the two sources
//! the migrations write. Operates on raw JSON maps — unknown keys in the
//! user's real settings.json are preserved verbatim.
//!
//! Same error contract as the proven `commands/core/effort.rs` port:
//! missing/empty file merges into an empty object; syntactically broken JSON
//! bails WITHOUT overwriting. (`effort.rs`/`permission::persist`/tools-meta carry
//! private copies of this logic; consolidating them here is a noted follow-up,
//! out of scope for this batch.)

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Which settings file to address (the migrations only write these two).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSource {
    /// `userSettings` → `<claude-config-home>/settings.json`.
    User,
    /// `localSettings` → `<project>/.claude/settings.local.json`.
    Local,
}

/// Resolve the file path for a source (TS `getSettingsFilePathForSource`).
#[must_use]
pub fn settings_path(source: SettingsSource, claude_home: &Path, project_dir: &Path) -> PathBuf {
    match source {
        SettingsSource::User => claude_home.join("settings.json"),
        SettingsSource::Local => project_dir.join(".claude").join("settings.local.json"),
    }
}

/// Raw read of a settings file. Missing / blank ⇒ empty map; broken JSON ⇒
/// `Err` (caller decides; migrations treat it as their TS catch path).
pub fn read_settings_map(path: &Path) -> Result<Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) if content.trim().is_empty() => Ok(Map::new()),
        Ok(content) => serde_json::from_str(&content)
            .map_err(|_| format!("Invalid JSON syntax in settings file at {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(format!(
            "Failed to read raw settings from {}: {e}",
            path.display()
        )),
    }
}

/// `updateSettingsForSource`: apply top-level key updates. `Some(v)` sets the
/// key, `None` deletes it (TS `mergeWith` treats `undefined` as delete).
/// All other keys preserved; pretty-printed + trailing newline.
pub fn update_settings(
    path: &Path,
    updates: Vec<(String, Option<Value>)>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let mut map = read_settings_map(path)?;
    for (key, value) in updates {
        match value {
            Some(v) => {
                map.insert(key, v);
            }
            None => {
                map.remove(&key);
            }
        }
    }
    let serialized = serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|e| format!("Failed to serialize settings for {}: {e}", path.display()))?;
    std::fs::write(path, serialized + "\n")
        .map_err(|e| format!("Failed to write settings to {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_config;

    #[test]
    fn paths_for_sources() {
        let t = temp_config();
        assert_eq!(
            settings_path(SettingsSource::User, &t.home, &t.project),
            t.home.join("settings.json")
        );
        assert_eq!(
            settings_path(SettingsSource::Local, &t.home, &t.project),
            t.project.join(".claude").join("settings.local.json")
        );
    }

    #[test]
    fn update_creates_file_and_merges_and_deletes() {
        let t = temp_config();
        let path = settings_path(SettingsSource::User, &t.home, &t.project);
        update_settings(&path, vec![("model".into(), Some(serde_json::json!("opus")))]).unwrap();
        update_settings(&path, vec![("other".into(), Some(serde_json::json!(1)))]).unwrap();
        let map = read_settings_map(&path).unwrap();
        assert_eq!(map["model"], serde_json::json!("opus"));
        assert_eq!(map["other"], serde_json::json!(1));

        update_settings(&path, vec![("model".into(), None)]).unwrap();
        let map = read_settings_map(&path).unwrap();
        assert!(map.get("model").is_none());
        assert_eq!(map["other"], serde_json::json!(1));
    }

    #[test]
    fn update_bails_on_broken_json_without_overwriting() {
        let t = temp_config();
        let path = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ broken").unwrap();
        let res = update_settings(&path, vec![("x".into(), Some(serde_json::json!(1)))]);
        assert!(res.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");
    }

    #[test]
    fn read_settings_map_missing_and_empty_are_empty() {
        let t = temp_config();
        let path = settings_path(SettingsSource::User, &t.home, &t.project);
        assert!(read_settings_map(&path).unwrap().is_empty());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "   \n").unwrap();
        assert!(read_settings_map(&path).unwrap().is_empty());
    }
}
