//! M7-15 Task 9 — best-effort theme persistence via `~/.claude/settings.json`
//! `theme` field. Round-trips through the explicit-path test seams.

use tui::theme::{ThemeName, ThemeSetting};
use tui::theme_persist;

#[test]
fn save_then_load_roundtrips_via_settings_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    // Save a setting to an explicit path (test-injected).
    theme_persist::save_theme_setting_to(&path, ThemeSetting::Named(ThemeName::Light)).unwrap();
    // The JSON object carries `"theme": "light"`.
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"theme\""));
    assert!(body.contains("\"light\""));
    // Load reads it back.
    let loaded = theme_persist::load_theme_setting_from(&path);
    assert_eq!(loaded, Some(ThemeSetting::Named(ThemeName::Light)));
}

#[test]
fn save_preserves_other_settings_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{\n  \"model\": \"claude-sonnet-4.5\"\n}\n").unwrap();
    theme_persist::save_theme_setting_to(&path, ThemeSetting::Named(ThemeName::Dark)).unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"model\"")); // untouched
    assert!(body.contains("\"theme\"")); // added
}

#[test]
fn load_missing_or_bad_file_is_none() {
    let dir = tempfile::tempdir().unwrap();
    // Missing file.
    let missing = dir.path().join("nope.json");
    assert_eq!(theme_persist::load_theme_setting_from(&missing), None);
    // Present file with no `theme` key.
    let no_theme = dir.path().join("no_theme.json");
    std::fs::write(&no_theme, "{\"model\":\"x\"}\n").unwrap();
    assert_eq!(theme_persist::load_theme_setting_from(&no_theme), None);
    // Present file with an unknown theme value.
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{\"theme\":\"bogus\"}\n").unwrap();
    assert_eq!(theme_persist::load_theme_setting_from(&bad), None);
}

#[test]
fn auto_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    theme_persist::save_theme_setting_to(&path, ThemeSetting::Auto).unwrap();
    assert_eq!(
        theme_persist::load_theme_setting_from(&path),
        Some(ThemeSetting::Auto)
    );
}

#[test]
fn syntax_highlighting_disabled_roundtrips_and_preserves_theme() {
    // (theme-missing-syntax-toggle)
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    // Seed an existing theme so we can prove the flag write preserves it.
    theme_persist::save_theme_setting_to(&path, ThemeSetting::Named(ThemeName::Dark)).unwrap();
    theme_persist::save_syntax_highlighting_disabled_to(&path, true).unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"syntaxHighlightingDisabled\""));
    assert!(body.contains("\"theme\"")); // theme untouched
    assert_eq!(
        theme_persist::load_syntax_highlighting_disabled_from(&path),
        Some(true)
    );
    assert_eq!(
        theme_persist::load_theme_setting_from(&path),
        Some(ThemeSetting::Named(ThemeName::Dark))
    );
}

#[test]
fn syntax_highlighting_disabled_absent_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{\"theme\":\"dark\"}\n").unwrap();
    assert_eq!(
        theme_persist::load_syntax_highlighting_disabled_from(&path),
        None
    );
}

#[test]
fn prefers_reduced_motion_loads_and_defaults_to_none() {
    // (SS-06)
    let dir = tempfile::tempdir().unwrap();
    let set = dir.path().join("on.json");
    std::fs::write(&set, "{\"prefersReducedMotion\":true}\n").unwrap();
    assert_eq!(
        theme_persist::load_prefers_reduced_motion_from(&set),
        Some(true)
    );
    let absent = dir.path().join("absent.json");
    std::fs::write(&absent, "{\"theme\":\"dark\"}\n").unwrap();
    assert_eq!(theme_persist::load_prefers_reduced_motion_from(&absent), None);
}
