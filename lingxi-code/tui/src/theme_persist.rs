//! Best-effort theme persistence via `~/.lingxi/settings.json` `theme` field.
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

/// Resolve `<config-home>/settings.json` (the same target the config tool uses):
/// `$LINGXI_CONFIG_DIR` when set, else `~/.claude`.
#[must_use]
fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| memory::lingxi_md::user_config_dir(&h).join("settings.json"))
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

/// (theme-missing-syntax-toggle) The settings key claude-code persists the
/// theme picker's Ctrl+T toggle under (`ThemePicker.tsx`'s
/// `syntaxHighlightingDisabled`).
const SYNTAX_DISABLED_KEY: &str = "syntaxHighlightingDisabled";

/// (theme-missing-syntax-toggle) Read the stored `syntaxHighlightingDisabled`
/// flag. `None` on any error / absent key (caller defaults to `false`).
#[must_use]
pub fn load_syntax_highlighting_disabled() -> Option<bool> {
    load_syntax_highlighting_disabled_from(&settings_path()?)
}

/// Test seam: read the flag from an explicit path.
#[must_use]
pub fn load_syntax_highlighting_disabled_from(path: &Path) -> Option<bool> {
    let body = std::fs::read_to_string(path).ok()?;
    let obj: Map<String, Value> = serde_json::from_str(&body).ok()?;
    obj.get(SYNTAX_DISABLED_KEY)?.as_bool()
}

/// (theme-missing-syntax-toggle) Best-effort save of the flag. Logs +
/// swallows errors (stays session-only on failure).
pub fn save_syntax_highlighting_disabled(disabled: bool) {
    let Some(path) = settings_path() else {
        tracing::debug!("syntax-highlighting-disabled persist skipped: no home dir");
        return;
    };
    if let Err(e) = save_syntax_highlighting_disabled_to(&path, disabled) {
        tracing::debug!(error = %e, "syntax-highlighting-disabled persist failed (session-only)");
    }
}

/// Test seam: read-modify-write the `syntaxHighlightingDisabled` field at an
/// explicit path, preserving all other keys (same JSON shape as
/// [`save_theme_setting_to`]).
pub fn save_syntax_highlighting_disabled_to(path: &Path, disabled: bool) -> std::io::Result<()> {
    let mut obj: Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    obj.insert(SYNTAX_DISABLED_KEY.to_string(), Value::Bool(disabled));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&obj)?;
    body.push('\n');
    std::fs::write(path, body)
}

/// (SS-06) The settings key claude-code reads the reduced-motion preference
/// from (`prefersReducedMotion`). Read-only here — LingXi has no UI to set it
/// (it's an app/OS-level accessibility preference in claude-code).
const REDUCED_MOTION_KEY: &str = "prefersReducedMotion";

/// (SS-06) Read the stored `prefersReducedMotion` flag. `None` on any error /
/// absent key (caller defaults to `false`).
#[must_use]
pub fn load_prefers_reduced_motion() -> Option<bool> {
    load_prefers_reduced_motion_from(&settings_path()?)
}

/// Test seam: read the flag from an explicit path.
#[must_use]
pub fn load_prefers_reduced_motion_from(path: &Path) -> Option<bool> {
    let body = std::fs::read_to_string(path).ok()?;
    let obj: Map<String, Value> = serde_json::from_str(&body).ok()?;
    obj.get(REDUCED_MOTION_KEY)?.as_bool()
}
