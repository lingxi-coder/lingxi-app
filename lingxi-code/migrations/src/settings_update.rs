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
    /// `localSettings` → `<project>/.lingxi/settings.local.json`.
    Local,
}

/// Resolve the file path for a source (TS `getSettingsFilePathForSource`).
#[must_use]
pub fn settings_path(source: SettingsSource, claude_home: &Path, project_dir: &Path) -> PathBuf {
    match source {
        SettingsSource::User => claude_home.join("settings.json"),
        SettingsSource::Local => project_dir
            .join(branding::DOT_DIR)
            .join("settings.local.json"),
    }
}

/// Raw read of a settings file. Missing / blank ⇒ empty map; broken JSON ⇒
/// `Err` (caller decides; in TS a broken file just yields `settings: null`
/// from `getSettingsForSource`, so migration callers generally treat `Err`
/// as "no settings" or warn-and-continue).
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
///
/// CALLER CONTRACT — top-level REPLACE, not deep-merge: TS uses a lodash
/// `mergeWith` (deep; arrays replace), but every migration pre-builds its
/// nested values (e.g. the spread-merged `env` map), so top-level replace
/// coincides for all current callers. A future caller passing a nested
/// partial would silently diverge — pre-merge at the call site.
///
/// DOCUMENTED DIVERGENCE — non-atomic write: TS routes settings through the
/// same atomic tmp+rename writer as the global config (`settings.ts:500-503`
/// → `writeFileSyncAndFlush_DEPRECATED`); this port uses an in-place
/// `std::fs::write`, following the workspace's `commands/core/effort.rs`
/// precedent. A torn write parses as broken JSON, which every reader/writer
/// here refuses to overwrite. Consolidating onto a shared atomic settings
/// writer is a noted follow-up.
///
/// Error contract: the TS original NEVER throws — every failure path returns
/// `{error: Error}` (settings.ts:416-523), and every migration discards that
/// return. So in the migration ports `Err` from this function maps to
/// warn-and-continue; only `global_config::save_map` failures (the
/// `saveGlobalConfig` analog, which CAN throw in TS) map to a TS catch path.
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
    // NO explicit mode: the TS settings write (`settings.ts:500-503`) passes
    // no `mode` option, so new files get umask-default permissions (same as
    // the workspace's `commands/core/effort.rs` writer). Only the
    // global-config path uses 0o600. Trailing `\n` IS settings-specific
    // (`+ '\n'`, `settings.ts:502`).
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
            t.project.join(".lingxi").join("settings.local.json")
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
