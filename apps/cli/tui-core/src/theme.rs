//! TUI color palette (M7-15 theme picker).
//!
//! M6-02 shipped four fixed `TuiTheme` consts. M7-15 expands this into a real
//! [`Theme`] value-struct + a registry of claude-code's 6 named themes
//! (`utils/theme.ts`): dark / light / light-daltonized / dark-daltonized /
//! light-ansi / dark-ansi. [`ThemeName`] is one of the 6 renderable themes;
//! [`ThemeSetting`] is the stored *preference* (`auto` + the 6 names). The
//! original `TuiTheme` consts remain as a thin shim delegating to
//! [`Theme::dark`] so any straggler M6 call site compiles unchanged.

#[cfg(test)]
use crate::render::{NamedColor, StyleColor};

/// Terminal color capability used to choose between the truecolor themes
/// (`Dark`/`Light`) and the 16-color `-ansi` themes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColorDepth {
    /// 24-bit color available — use the rgb themes.
    Truecolor,
    /// 256/16-color (or unknown) — use the `-ansi` themes to avoid the terminal
    /// quantizing truecolor SGR to the wrong nearest ANSI slot.
    Low,
}

/// Pure color-depth classification from `$COLORTERM` + `$TERM`.
pub(crate) fn color_depth_from(colorterm: Option<&str>, term: Option<&str>) -> ColorDepth {
    if matches!(colorterm, Some("truecolor") | Some("24bit")) {
        return ColorDepth::Truecolor;
    }
    if let Some(t) = term {
        if t.contains("direct") || t.contains("truecolor") {
            return ColorDepth::Truecolor;
        }
    }
    ColorDepth::Low
}

/// Color depth from the live environment.
pub(crate) fn color_depth() -> ColorDepth {
    color_depth_from(
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}

pub use client::presentation::theme::{ansi, theme_for, Theme, ThemeName, TuiTheme};

/// A theme *preference* as stored in config. `Auto` follows the terminal and
/// resolves to a [`ThemeName`] at runtime (claude-code `ThemeSetting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeSetting {
    /// Match the terminal background (resolves to [`ThemeName::Dark`] headless).
    Auto,
    /// A concrete named theme.
    Named(ThemeName),
}

impl ThemeSetting {
    /// `auto` + the 6 names, in `THEME_SETTINGS` order.
    pub const ALL: [ThemeSetting; 7] = [
        ThemeSetting::Auto,
        ThemeSetting::Named(ThemeName::Dark),
        ThemeSetting::Named(ThemeName::Light),
        ThemeSetting::Named(ThemeName::LightDaltonized),
        ThemeSetting::Named(ThemeName::DarkDaltonized),
        ThemeSetting::Named(ThemeName::LightAnsi),
        ThemeSetting::Named(ThemeName::DarkAnsi),
    ];

    /// Wire string, byte-for-byte with claude-code's `ThemeSetting`.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            ThemeSetting::Auto => "auto",
            ThemeSetting::Named(n) => n.as_wire(),
        }
    }

    /// Parse a wire string back to a `ThemeSetting`. `None` for unknown values.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<ThemeSetting> {
        if s == "auto" {
            return Some(ThemeSetting::Auto);
        }
        ThemeName::from_wire(s).map(ThemeSetting::Named)
    }

    /// Resolve to a concrete renderable theme. `Auto` consults the
    /// process-global OSC-11 detection cache (set at startup), then
    /// `$COLORFGBG` (claude-code `detectFromColorFgBg`) as a fallback, and
    /// finally defaults to `Dark`. Color depth then selects the truecolor or
    /// `-ansi` variant of the chosen background.
    #[must_use]
    pub fn resolve(self) -> ThemeName {
        match self {
            ThemeSetting::Auto => resolve_auto(
                crate::theme_detect::detected_background(),
                std::env::var("COLORFGBG").ok().as_deref(),
                color_depth(),
            ),
            ThemeSetting::Named(n) => n,
        }
    }
}

/// Select the concrete theme from a detected background and color depth.
/// `background` is only ever `Light` or `Dark`; the `_` arm covers `Dark`.
pub(crate) fn resolve_theme(background: ThemeName, depth: ColorDepth) -> ThemeName {
    match (background, depth) {
        (ThemeName::Light, ColorDepth::Truecolor) => ThemeName::Light,
        (ThemeName::Light, ColorDepth::Low) => ThemeName::LightAnsi,
        (_, ColorDepth::Truecolor) => ThemeName::Dark,
        (_, ColorDepth::Low) => ThemeName::DarkAnsi,
    }
}

/// Pure `Auto` resolution: detected background → `$COLORFGBG` → Dark, combined
/// with color depth. Extracted from `resolve()` so the precedence is testable
/// without touching process-global state or the environment.
pub(crate) fn resolve_auto(
    detected: Option<ThemeName>,
    colorfgbg: Option<&str>,
    depth: ColorDepth,
) -> ThemeName {
    let background = detected
        .or_else(|| colorfgbg_theme(colorfgbg))
        .unwrap_or(ThemeName::Dark);
    resolve_theme(background, depth)
}

/// (theme-02) claude-code `detectFromColorFgBg`: parse `$COLORFGBG`
/// (`fg;bg` or `fg;other;bg`) and classify by the LAST `;`-delimited
/// component, an ANSI color index 0..=15. `0..=6` and `8` are dark ANSI
/// colors; `7` (white) and `9..=15` (bright) are light. `None` when the
/// input is missing, has no last segment, or that segment isn't an integer
/// in `0..=15`.
#[must_use]
fn colorfgbg_theme(colorfgbg: Option<&str>) -> Option<ThemeName> {
    let bg = colorfgbg?.split(';').next_back()?;
    if bg.is_empty() {
        return None;
    }
    let bg_num: i32 = bg.parse().ok()?;
    if !(0..=15).contains(&bg_num) {
        return None;
    }
    Some(if bg_num <= 6 || bg_num == 8 {
        ThemeName::Dark
    } else {
        ThemeName::Light
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_is_claude_accent() {
        assert_eq!(TuiTheme::ASSISTANT, Theme::dark().claude);
    }

    #[test]
    fn user_is_reset() {
        assert!(matches!(TuiTheme::USER, StyleColor::Default));
    }

    #[test]
    fn error_is_theme_error() {
        assert_eq!(TuiTheme::ERROR, Theme::dark().error);
    }

    #[test]
    fn dim_is_theme_dim() {
        assert_eq!(TuiTheme::DIM, Theme::dark().dim);
    }

    #[test]
    fn theme_names_count_and_wire_roundtrip() {
        // 6 renderable themes.
        assert_eq!(ThemeName::ALL.len(), 6);
        // Wire roundtrip for every setting (auto + 6 names).
        for s in ThemeSetting::ALL {
            assert_eq!(ThemeSetting::from_wire(s.as_wire()), Some(s));
        }
        // Exact wire strings (claude-code parity).
        assert_eq!(ThemeSetting::Auto.as_wire(), "auto");
        assert_eq!(ThemeName::Dark.as_wire(), "dark");
        assert_eq!(ThemeName::DarkDaltonized.as_wire(), "dark-daltonized");
        assert_eq!(ThemeName::LightAnsi.as_wire(), "light-ansi");
        assert_eq!(ThemeSetting::from_wire("bogus"), None);
    }

    #[test]
    fn ansi_map_covers_bright_and_dark() {
        assert!(matches!(
            ansi("ansi:red"),
            StyleColor::Named(NamedColor::Red)
        ));
        assert!(matches!(
            ansi("ansi:redBright"),
            StyleColor::Named(NamedColor::BrightRed)
        ));
        assert!(matches!(
            ansi("ansi:white"),
            StyleColor::Named(NamedColor::White)
        ));
        assert!(matches!(
            ansi("ansi:whiteBright"),
            StyleColor::Named(NamedColor::BrightWhite)
        ));
        assert!(matches!(
            ansi("ansi:black"),
            StyleColor::Named(NamedColor::Black)
        ));
        assert!(matches!(
            ansi("ansi:blackBright"),
            StyleColor::Named(NamedColor::BrightBlack)
        ));
    }

    #[test]
    fn named_resolves_to_itself() {
        // Env-independent: Named never reads $COLORFGBG.
        assert_eq!(
            ThemeSetting::Named(ThemeName::Light).resolve(),
            ThemeName::Light
        );
        assert_eq!(
            ThemeSetting::Named(ThemeName::Dark).resolve(),
            ThemeName::Dark
        );
    }

    #[test]
    fn colorfgbg_theme_variants() {
        // (theme-02) Pure helper — no env mutation, so no race with other
        // tests/processes that might have $COLORFGBG set for real.
        assert_eq!(colorfgbg_theme(None), None);
        assert_eq!(colorfgbg_theme(Some("")), None);
        assert_eq!(colorfgbg_theme(Some("not-a-number")), None);
        assert_eq!(colorfgbg_theme(Some("16")), None); // out of 0..=15 range
        assert_eq!(colorfgbg_theme(Some("-1")), None);
        // rxvt `fg;bg` form.
        assert_eq!(colorfgbg_theme(Some("15;0")), Some(ThemeName::Dark));
        assert_eq!(colorfgbg_theme(Some("0;8")), Some(ThemeName::Dark));
        assert_eq!(colorfgbg_theme(Some("0;7")), Some(ThemeName::Light));
        assert_eq!(colorfgbg_theme(Some("0;15")), Some(ThemeName::Light));
        // `fg;other;bg` form — last segment wins.
        assert_eq!(colorfgbg_theme(Some("15;default;0")), Some(ThemeName::Dark));
    }

    #[test]
    fn color_depth_detection() {
        assert_eq!(
            color_depth_from(Some("truecolor"), None),
            ColorDepth::Truecolor
        );
        assert_eq!(color_depth_from(Some("24bit"), None), ColorDepth::Truecolor);
        assert_eq!(
            color_depth_from(None, Some("xterm-direct")),
            ColorDepth::Truecolor
        );
        assert_eq!(
            color_depth_from(None, Some("xterm-256color")),
            ColorDepth::Low
        );
        assert_eq!(color_depth_from(None, Some("screen")), ColorDepth::Low);
        assert_eq!(color_depth_from(None, None), ColorDepth::Low);
    }

    #[test]
    fn auto_falls_back_to_dark_without_colorfgbg() {
        // Best-effort: in a test environment without $COLORFGBG set, Auto
        // resolves to the documented dark fallback. (If a developer's real
        // shell happens to export $COLORFGBG, this assertion would reflect
        // that — acceptable, since the pure-helper test above is what
        // actually locks the parsing behavior.) After theme-03 the result
        // is also shaped by color depth: Dark in truecolor environments,
        // DarkAnsi in 16-color ones — both are the "dark" family.
        if std::env::var("COLORFGBG").is_err() {
            let resolved = ThemeSetting::Auto.resolve();
            assert!(
                resolved == ThemeName::Dark || resolved == ThemeName::DarkAnsi,
                "expected Dark or DarkAnsi without $COLORFGBG, got {resolved:?}"
            );
        }
    }

    #[test]
    fn resolve_theme_four_cells() {
        use ColorDepth::*;
        assert_eq!(resolve_theme(ThemeName::Light, Truecolor), ThemeName::Light);
        assert_eq!(resolve_theme(ThemeName::Light, Low), ThemeName::LightAnsi);
        assert_eq!(resolve_theme(ThemeName::Dark, Truecolor), ThemeName::Dark);
        assert_eq!(resolve_theme(ThemeName::Dark, Low), ThemeName::DarkAnsi);
    }

    #[test]
    fn resolve_auto_precedence() {
        use ColorDepth::Truecolor;
        // Detected background wins over COLORFGBG.
        assert_eq!(
            resolve_auto(Some(ThemeName::Light), Some("0;15"), Truecolor),
            ThemeName::Light
        );
        // No detection → COLORFGBG light (bg index 15) wins over the Dark default.
        assert_eq!(
            resolve_auto(None, Some("0;15"), Truecolor),
            ThemeName::Light
        );
        // No detection, no COLORFGBG → Dark.
        assert_eq!(resolve_auto(None, None, Truecolor), ThemeName::Dark);
        // Low color depth degrades to the -ansi theme.
        assert_eq!(
            resolve_auto(None, None, ColorDepth::Low),
            ThemeName::DarkAnsi
        );
    }

    #[test]
    fn registry_returns_distinct_palettes_with_locked_colors() {
        let dark = theme_for(ThemeName::Dark);
        let light = theme_for(ThemeName::Light);
        // dark.text = rgb(255,255,255); light.text = rgb(0,0,0)  (theme.ts).
        assert_eq!(dark.text, StyleColor::Rgb(255, 255, 255));
        assert_eq!(light.text, StyleColor::Rgb(0, 0, 0));
        // dark.error = rgb(255,107,128); light.error = rgb(171,43,63).
        assert_eq!(dark.error, StyleColor::Rgb(255, 107, 128));
        assert_eq!(light.error, StyleColor::Rgb(171, 43, 63));
        // dark.claude = rgb(215,119,87)  (the Claude orange accent).
        assert_eq!(dark.claude, StyleColor::Rgb(215, 119, 87));
        // ANSI theme uses named colors, not Rgb.
        let dark_ansi = theme_for(ThemeName::DarkAnsi);
        assert_eq!(dark_ansi.error, StyleColor::Named(NamedColor::BrightRed)); // ansi:redBright
        assert_eq!(
            dark_ansi.success,
            StyleColor::Named(NamedColor::BrightGreen)
        ); // ansi:greenBright
           // Every theme resolves (no panic) and is Copy.
        for n in ThemeName::ALL {
            let _t: Theme = theme_for(n);
        }
    }
}
