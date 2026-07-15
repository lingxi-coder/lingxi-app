//! Best-effort theme persistence via `~/.lingxi/settings.json` `theme` field.
//!
//! No new persistence engine (spec §4 R7): this read-modify-writes the same
//! JSON object the existing `ConfigTool` allowlists (the `theme` field is
//! already an allowlisted config field — `crates/tools/.../config.rs`
//! `CONFIG_FIELD_THEME`). If the home dir or file is unavailable, save/load
//! degrade to a no-op and the theme stays session-only — the picker still
//! applies it live.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
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

/// The `verbose` config field (allowlisted by the config tool,
/// `CONFIG_FIELD_VERBOSE`). Persisted by `/config verbose=…` so the transcript
/// verbose mode survives restarts; read back at startup.
const VERBOSE_KEY: &str = "verbose";

/// Read the stored `verbose` flag. `None` on any error / absent key.
#[must_use]
pub fn load_verbose() -> Option<bool> {
    load_bool_field_from(&settings_path()?, VERBOSE_KEY)
}

/// Best-effort save of `verbose`. Logs + swallows errors (session-only on fail).
pub fn save_verbose(verbose: bool) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Err(e) = save_bool_field_to(&path, VERBOSE_KEY, verbose) {
        tracing::debug!(error = %e, "verbose persist failed (session-only)");
    }
}

/// The `editorMode` config field (`"vim"` | `"normal"`), the key claude-code
/// persists the composer's Vim mode under. Persisted by `/config vim=…`; read
/// back at startup.
const EDITOR_MODE_KEY: &str = "editorMode";

/// The `vimInsertModeRemaps` config field added in Claude Code 2.1.208. Shape:
/// `{ "jj": "Escape" }`. Only two-character sequences are meaningful.
const VIM_INSERT_MODE_REMAPS_KEY: &str = "vimInsertModeRemaps";

/// Read the stored editor mode. `Some(true)` ⇒ Vim, `Some(false)` ⇒ normal,
/// `None` ⇒ unset/error.
#[must_use]
pub fn load_editor_mode_is_vim() -> Option<bool> {
    load_editor_mode_is_vim_from(&settings_path()?)
}

/// Test seam: read the editor mode from an explicit path.
#[must_use]
pub fn load_editor_mode_is_vim_from(path: &Path) -> Option<bool> {
    let body = std::fs::read_to_string(path).ok()?;
    let obj: Map<String, Value> = serde_json::from_str(&body).ok()?;
    Some(obj.get(EDITOR_MODE_KEY)?.as_str()? == "vim")
}

/// Read configured Vim insert-mode remaps. Unknown/non-string entries and
/// sequences that are not exactly two chars are ignored.
#[must_use]
pub fn load_vim_insert_mode_remaps() -> Option<BTreeMap<String, String>> {
    load_vim_insert_mode_remaps_from(&settings_path()?)
}

/// Test seam: read Vim insert-mode remaps from an explicit settings path.
#[must_use]
pub fn load_vim_insert_mode_remaps_from(path: &Path) -> Option<BTreeMap<String, String>> {
    let body = std::fs::read_to_string(path).ok()?;
    let obj: Map<String, Value> = serde_json::from_str(&body).ok()?;
    let remaps = obj.get(VIM_INSERT_MODE_REMAPS_KEY)?.as_object()?;
    let parsed: BTreeMap<String, String> = remaps
        .iter()
        .filter_map(|(from, to)| {
            let to = to.as_str()?;
            (from.chars().count() == 2).then(|| (from.clone(), to.to_string()))
        })
        .collect();
    (!parsed.is_empty()).then_some(parsed)
}

/// Best-effort save of the editor mode (`vim` ⇒ `"vim"`, else `"normal"`).
pub fn save_editor_mode(vim: bool) {
    let Some(path) = settings_path() else {
        return;
    };
    let value = if vim { "vim" } else { "normal" };
    if let Err(e) = save_string_field_to(&path, EDITOR_MODE_KEY, value) {
        tracing::debug!(error = %e, "editorMode persist failed (session-only)");
    }
}

/// The `workflowSizeGuideline` config field (parity 2.1.207): the `/config`
/// "Dynamic workflow size" setting — one of `unrestricted` / `small` / `medium`
/// / `large`. Persisted by `/config workflowSizeGuideline=…`; read back at
/// startup so the Workflow tool's prompt appendix survives restarts.
const WORKFLOW_SIZE_GUIDELINE_KEY: &str = "workflowSizeGuideline";

/// Read the stored `workflowSizeGuideline` wire string. `None` on any error /
/// absent key (caller treats absence as `unrestricted`).
#[must_use]
pub fn load_workflow_size_guideline() -> Option<String> {
    load_workflow_size_guideline_from(&settings_path()?)
}

/// Test seam: read the guideline from an explicit path.
#[must_use]
pub fn load_workflow_size_guideline_from(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    let obj: Map<String, Value> = serde_json::from_str(&body).ok()?;
    Some(obj.get(WORKFLOW_SIZE_GUIDELINE_KEY)?.as_str()?.to_string())
}

/// Best-effort save of `workflowSizeGuideline`. Logs + swallows errors
/// (session-only on failure).
pub fn save_workflow_size_guideline(value: &str) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Err(e) = save_string_field_to(&path, WORKFLOW_SIZE_GUIDELINE_KEY, value) {
        tracing::debug!(error = %e, "workflowSizeGuideline persist failed (session-only)");
    }
}

/// Shared read: a top-level bool field at an explicit path.
#[must_use]
fn load_bool_field_from(path: &Path, key: &str) -> Option<bool> {
    let body = std::fs::read_to_string(path).ok()?;
    let obj: Map<String, Value> = serde_json::from_str(&body).ok()?;
    obj.get(key)?.as_bool()
}

/// Shared write: read-modify-write a top-level bool field, preserving all other
/// keys (same JSON shape as [`save_theme_setting_to`]).
fn save_bool_field_to(path: &Path, key: &str, value: bool) -> std::io::Result<()> {
    write_field(path, key, Value::Bool(value))
}

/// Shared write: read-modify-write a top-level string field.
fn save_string_field_to(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
    write_field(path, key, Value::String(value.to_string()))
}

/// The read-modify-write core shared by every field writer here.
fn write_field(path: &Path, key: &str, value: Value) -> std::io::Result<()> {
    let mut obj: Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    obj.insert(key.to_string(), value);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_and_editor_mode_round_trip_preserving_other_keys() {
        let dir = std::env::temp_dir().join(format!("lingxi_cfg_persist_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let _ = std::fs::remove_file(&path);

        // verbose (bool) round-trips.
        save_bool_field_to(&path, VERBOSE_KEY, true).unwrap();
        assert_eq!(load_bool_field_from(&path, VERBOSE_KEY), Some(true));
        save_bool_field_to(&path, VERBOSE_KEY, false).unwrap();
        assert_eq!(load_bool_field_from(&path, VERBOSE_KEY), Some(false));

        // editorMode (string) → vim/normal.
        save_string_field_to(&path, EDITOR_MODE_KEY, "vim").unwrap();
        assert_eq!(load_editor_mode_is_vim_from(&path), Some(true));
        save_string_field_to(&path, EDITOR_MODE_KEY, "normal").unwrap();
        assert_eq!(load_editor_mode_is_vim_from(&path), Some(false));

        // A later write preserves the earlier keys (read-modify-write).
        save_theme_setting_to(&path, ThemeSetting::Auto).unwrap();
        assert_eq!(load_bool_field_from(&path, VERBOSE_KEY), Some(false));
        assert_eq!(load_editor_mode_is_vim_from(&path), Some(false));

        // Absent key → None.
        assert_eq!(load_bool_field_from(&path, "nope"), None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn workflow_size_guideline_round_trips_preserving_other_keys() {
        let dir = std::env::temp_dir().join(format!("lingxi_wsg_persist_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let _ = std::fs::remove_file(&path);

        // Absent → None (treated as unrestricted by the caller).
        assert_eq!(load_workflow_size_guideline_from(&path), None);

        // Seed another key, then round-trip the guideline; the earlier key survives.
        save_string_field_to(&path, EDITOR_MODE_KEY, "vim").unwrap();
        for want in ["small", "medium", "large", "unrestricted"] {
            save_string_field_to(&path, WORKFLOW_SIZE_GUIDELINE_KEY, want).unwrap();
            assert_eq!(
                load_workflow_size_guideline_from(&path).as_deref(),
                Some(want)
            );
        }
        assert_eq!(load_editor_mode_is_vim_from(&path), Some(true));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vim_insert_mode_remaps_parse_two_key_sequences() {
        let dir =
            std::env::temp_dir().join(format!("lingxi_vim_remaps_persist_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{
  "vimInsertModeRemaps": {
    "jj": "Escape",
    "jk": "Esc",
    "x": "Escape",
    "long": "Escape",
    "nope": false
  }
}
"#,
        )
        .unwrap();

        let remaps = load_vim_insert_mode_remaps_from(&path).expect("remaps");
        assert_eq!(remaps.get("jj").map(String::as_str), Some("Escape"));
        assert_eq!(remaps.get("jk").map(String::as_str), Some("Esc"));
        assert!(!remaps.contains_key("x"));
        assert!(!remaps.contains_key("long"));
        assert!(!remaps.contains_key("nope"));
        let _ = std::fs::remove_file(&path);
    }
}
