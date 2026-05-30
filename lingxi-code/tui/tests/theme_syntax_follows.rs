//! M7-15 Task 6 — syntect `.tmTheme` follows the active `ThemeName`.

use tui::render::syntax;
use tui::theme::ThemeName;

#[test]
fn syntect_theme_changes_with_active_theme() {
    // Same code, two themes → different highlighted output.
    let code = "fn main() { let x = 1; }";
    let dark = syntax::highlight(code, Some("rust"), ThemeName::Dark);
    let light = syntax::highlight(code, Some("rust"), ThemeName::Light);
    // The two renders are not byte-identical (different tmTheme palettes).
    assert_ne!(
        format!("{dark:?}"),
        format!("{light:?}"),
        "dark and light highlight should differ"
    );
}

#[test]
fn tm_theme_for_maps_dark_and_light_to_different_themes() {
    let d = syntax::tm_theme_for(ThemeName::Dark);
    let l = syntax::tm_theme_for(ThemeName::Light);
    // syntect themes expose a `name`; the bundled dark/light themes differ.
    assert_ne!(d.name, l.name);
}

#[test]
fn ansi_themes_reuse_dark_light_tm_themes() {
    // ANSI themes map to the dark/light tmThemes (terminal handles ANSI palette).
    assert_eq!(
        syntax::tm_theme_for(ThemeName::DarkAnsi).name,
        syntax::tm_theme_for(ThemeName::Dark).name
    );
    assert_eq!(
        syntax::tm_theme_for(ThemeName::LightAnsi).name,
        syntax::tm_theme_for(ThemeName::Light).name
    );
}
