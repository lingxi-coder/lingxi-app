//! Best-effort theme persistence via `~/.claude/settings.json` `theme` field.
//!
//! No new persistence engine (spec §4 R7): this read-modify-writes the same
//! JSON object the existing `ConfigTool` allowlists (the `theme` field is
//! already an allowlisted config field — `crates/tools/.../config.rs`
//! `CONFIG_FIELD_THEME`). If the home dir or file is unavailable, save/load
//! degrade to a no-op and the theme stays session-only — the picker still
//! applies it live.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::theme::ThemeSetting;

/// Resolve `~/.claude/settings.json` (the same target the config tool uses).
#[must_use]
fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

/// Read the stored theme setting, if any. Returns `None` on any error
/// (missing file, parse failure, absent/unknown `theme` value).
#[must_use]
pub fn load_theme_setting() -> Option<ThemeSetting> {
    load_theme_setting_from(&settings_path()?)
}

/// Test seam: read from an explicit path.
#[must_use]
pub fn load_theme_setting_from(path: &Path) -> Option<ThemeSetting> {
    let body = std::fs::read_to_string(path).ok()?;
    let obj: Map<String, Value> = serde_json::from_str(&body).ok()?;
    let wire = obj.get("theme")?.as_str()?;
    ThemeSetting::from_wire(wire)
}

/// Best-effort save. Logs + swallows errors (theme stays session-only).
pub fn save_theme_setting(setting: ThemeSetting) {
    let Some(path) = settings_path() else {
        tracing::debug!("theme persist skipped: no home dir");
        return;
    };
    if let Err(e) = save_theme_setting_to(&path, setting) {
        tracing::debug!(error = %e, "theme persist failed (session-only)");
    }
}

/// Test seam: read-modify-write the `theme` field at an explicit path,
/// preserving all other keys. Pretty JSON + trailing newline (config-tool
/// shape).
pub fn save_theme_setting_to(path: &Path, setting: ThemeSetting) -> std::io::Result<()> {
    let mut obj: Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    obj.insert(
        "theme".to_string(),
        Value::String(setting.as_wire().to_string()),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&obj)?;
    body.push('\n');
    std::fs::write(path, body)
}
