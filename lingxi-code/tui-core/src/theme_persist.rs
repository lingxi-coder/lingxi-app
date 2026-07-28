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

/// The canonical Vim insert-mode remap target claude-code stores. Every
/// accepted entry maps to this exact string (`GGy` in the 2.1.208 binary:
/// `t.set(o,"<Esc>")`), so the composer never has to case-fold at match time.
pub const VIM_INSERT_REMAP_ESCAPE_TARGET: &str = "<Esc>";

/// Canonicalize raw `vimInsertModeRemaps` entries exactly the way claude-code
/// 2.1.208's `GGy` does. Each entry is kept only when:
/// * the target string case-insensitively equals `"<esc>"` (the only supported
///   target — `n.toLowerCase()!=="<esc>"` is skipped), and
/// * the NFC-normalized source is exactly two printable characters — two code
///   points that are neither control (`\p{C}`) nor separators (`\p{Z}`,
///   approximated with control/whitespace here) — and also exactly two grapheme
///   clusters (`/^[^\p{C}\p{Z}]{2}$/u` plus `Kse(o)===2`).
///
/// Surviving entries are stored under their NFC key mapping to the canonical
/// [`VIM_INSERT_REMAP_ESCAPE_TARGET`].
#[must_use]
pub fn canonicalize_vim_insert_mode_remaps<I>(entries: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    use unicode_normalization::UnicodeNormalization;
    use unicode_segmentation::UnicodeSegmentation;

    let mut out = BTreeMap::new();
    for (from, to) in entries {
        if !to.eq_ignore_ascii_case("<esc>") {
            continue;
        }
        let normalized: String = from.nfc().collect();
        // `/^[^\p{C}\p{Z}]{2}$/u`: exactly two code points, none control/space.
        let mut chars = normalized.chars();
        let (Some(a), Some(b), None) = (chars.next(), chars.next(), chars.next()) else {
            continue;
        };
        if is_forbidden_remap_char(a) || is_forbidden_remap_char(b) {
            continue;
        }
        // `Kse(o)===2`: exactly two grapheme clusters.
        if normalized.graphemes(true).count() != 2 {
            continue;
        }
        out.insert(normalized, VIM_INSERT_REMAP_ESCAPE_TARGET.to_string());
    }
    out
}

/// Whether `c` is excluded from a remap source by claude-code's
/// `[^\p{C}\p{Z}]` class. `\p{Z}` (all separators) is a subset of Rust's
/// `is_whitespace`; `\p{C}`'s control category is `is_control`. (Rust std can
/// not see the `\p{C}` format/private-use/unassigned subcategories, an accepted
/// approximation for two-key remaps.)
fn is_forbidden_remap_char(c: char) -> bool {
    c.is_control() || c.is_whitespace()
}

/// Read configured Vim insert-mode remaps. Entries are validated exactly like
/// claude-code 2.1.208's `GGy` via [`canonicalize_vim_insert_mode_remaps`].
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
    let parsed = canonicalize_vim_insert_mode_remaps(
        remaps
            .iter()
            .filter_map(|(from, to)| Some((from.clone(), to.as_str()?.to_string()))),
    );
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

/// Read the user-scope `workflowSizeGuideline` wire string. `None` on any
/// error / absent key. The canonical settings composition applies the
/// `medium` default and higher-precedence project/flag/managed layers.
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

/// The `leftArrowOpensAgents` config field (claude-code 2.1.220
/// `kCt = Rt().leftArrowOpensAgents !== false`): whether ← on an empty
/// composer opens the agents view. Absent ⇒ enabled (the `!== false` default).
const LEFT_ARROW_OPENS_AGENTS_KEY: &str = "leftArrowOpensAgents";

/// The `defaultToAgentsView` config field (claude-code's `/config` row
/// "Open agents view by default" / settings row "Start in agent view"):
/// whether a new session starts in the agents view. Absent ⇒ `false`
/// (the oracle's `?? !1`).
const DEFAULT_TO_AGENTS_VIEW_KEY: &str = "defaultToAgentsView";

/// Full-screen mouse-up copies the selected text unless explicitly disabled.
/// Inline mode never captures the mouse and therefore ignores this setting.
const COPY_ON_SELECT_KEY: &str = "copyOnSelect";

/// Read the stored `leftArrowOpensAgents` flag. `None` on any error / absent
/// key (caller defaults to `true`, mirroring `!== false`).
#[must_use]
pub fn load_left_arrow_opens_agents() -> Option<bool> {
    load_bool_field_from(&settings_path()?, LEFT_ARROW_OPENS_AGENTS_KEY)
}

/// Test seam: read the flag from an explicit path.
#[must_use]
pub fn load_left_arrow_opens_agents_from(path: &Path) -> Option<bool> {
    load_bool_field_from(path, LEFT_ARROW_OPENS_AGENTS_KEY)
}

/// Best-effort save of `leftArrowOpensAgents`. Logs + swallows errors
/// (session-only on failure).
pub fn save_left_arrow_opens_agents(enabled: bool) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Err(e) = save_bool_field_to(&path, LEFT_ARROW_OPENS_AGENTS_KEY, enabled) {
        tracing::debug!(error = %e, "leftArrowOpensAgents persist failed (session-only)");
    }
}

/// Test seam: write the flag at an explicit path.
pub fn save_left_arrow_opens_agents_to(path: &Path, enabled: bool) -> std::io::Result<()> {
    save_bool_field_to(path, LEFT_ARROW_OPENS_AGENTS_KEY, enabled)
}

/// Read `copyOnSelect`; callers default an absent key to `true`.
#[must_use]
pub fn load_copy_on_select() -> Option<bool> {
    load_copy_on_select_from(&settings_path()?)
}

/// Test seam for reading `copyOnSelect`.
#[must_use]
pub fn load_copy_on_select_from(path: &Path) -> Option<bool> {
    load_bool_field_from(path, COPY_ON_SELECT_KEY)
}

/// Persist the full-screen copy-on-select preference best-effort.
pub fn save_copy_on_select(enabled: bool) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Err(e) = save_copy_on_select_to(&path, enabled) {
        tracing::debug!(error = %e, "copyOnSelect persist failed (session-only)");
    }
}

/// Test seam for writing `copyOnSelect`.
pub fn save_copy_on_select_to(path: &Path, enabled: bool) -> std::io::Result<()> {
    save_bool_field_to(path, COPY_ON_SELECT_KEY, enabled)
}

/// Read the stored `defaultToAgentsView` flag. `None` on any error / absent
/// key (caller defaults to `false`).
#[must_use]
pub fn load_default_to_agents_view() -> Option<bool> {
    load_bool_field_from(&settings_path()?, DEFAULT_TO_AGENTS_VIEW_KEY)
}

/// Test seam: read the flag from an explicit path.
#[must_use]
pub fn load_default_to_agents_view_from(path: &Path) -> Option<bool> {
    load_bool_field_from(path, DEFAULT_TO_AGENTS_VIEW_KEY)
}

/// Best-effort save of `defaultToAgentsView`. Logs + swallows errors
/// (session-only on failure).
pub fn save_default_to_agents_view(enabled: bool) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Err(e) = save_bool_field_to(&path, DEFAULT_TO_AGENTS_VIEW_KEY, enabled) {
        tracing::debug!(error = %e, "defaultToAgentsView persist failed (session-only)");
    }
}

/// Test seam: write the flag at an explicit path.
pub fn save_default_to_agents_view_to(path: &Path, enabled: bool) -> std::io::Result<()> {
    save_bool_field_to(path, DEFAULT_TO_AGENTS_VIEW_KEY, enabled)
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

        // Absent → None (the canonical settings composition supplies medium).
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
    fn agents_view_flags_round_trip_preserving_other_keys() {
        let dir = std::env::temp_dir().join(format!("lingxi_agv_persist_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let _ = std::fs::remove_file(&path);

        // Absent → None (callers default `leftArrowOpensAgents` to true —
        // the oracle's `!== false` — and `defaultToAgentsView` to false).
        assert_eq!(load_left_arrow_opens_agents_from(&path), None);
        assert_eq!(load_default_to_agents_view_from(&path), None);
        assert_eq!(load_copy_on_select_from(&path), None);

        // Seed another key, then round-trip both flags; the earlier key survives.
        save_string_field_to(&path, EDITOR_MODE_KEY, "vim").unwrap();
        save_left_arrow_opens_agents_to(&path, false).unwrap();
        assert_eq!(load_left_arrow_opens_agents_from(&path), Some(false));
        save_default_to_agents_view_to(&path, true).unwrap();
        assert_eq!(load_default_to_agents_view_from(&path), Some(true));
        save_copy_on_select_to(&path, false).unwrap();
        assert_eq!(load_copy_on_select_from(&path), Some(false));
        assert_eq!(load_left_arrow_opens_agents_from(&path), Some(false));
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
    "jj": "<Esc>",
    "jk": "<ESC>",
    "kj": "Escape",
    "x": "<Esc>",
    "long": "<Esc>",
    "j k": "<Esc>",
    "nope": false
  }
}
"#,
        )
        .unwrap();

        let remaps = load_vim_insert_mode_remaps_from(&path).expect("remaps");
        // `<Esc>` / `<ESC>` accepted, canonicalized to `<Esc>`.
        assert_eq!(remaps.get("jj").map(String::as_str), Some("<Esc>"));
        assert_eq!(remaps.get("jk").map(String::as_str), Some("<Esc>"));
        // Non-`<esc>` target dropped (CC keeps only `<esc>`).
        assert!(!remaps.contains_key("kj"));
        // Wrong grapheme count dropped.
        assert!(!remaps.contains_key("x"));
        assert!(!remaps.contains_key("long"));
        // Source containing a separator (`\p{Z}`) dropped.
        assert!(!remaps.contains_key("j k"));
        // Non-string target dropped.
        assert!(!remaps.contains_key("nope"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn canonicalize_vim_remaps_matches_ggy_validation() {
        // NFC-normalizes the key: "e\u{0301}j" (e + combining acute) folds to
        // "éj" (2 code points, 2 graphemes) and is kept.
        let remaps = canonicalize_vim_insert_mode_remaps([
            ("e\u{0301}j".to_string(), "<esc>".to_string()),
            ("jj".to_string(), "<Esc>".to_string()),
        ]);
        assert_eq!(remaps.get("éj").map(String::as_str), Some("<Esc>"));
        assert_eq!(remaps.get("jj").map(String::as_str), Some("<Esc>"));

        // "a\u{0327}" (a + combining cedilla) has no precomposed form, so it
        // stays 2 code points after NFC but is a single grapheme -> dropped.
        let combined =
            canonicalize_vim_insert_mode_remaps([("a\u{0327}".to_string(), "<Esc>".to_string())]);
        assert!(combined.is_empty());
    }
}
