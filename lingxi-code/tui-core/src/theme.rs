//! TUI color palette (M7-15 theme picker).
//!
//! M6-02 shipped four fixed `TuiTheme` consts. M7-15 expands this into a real
//! [`Theme`] value-struct + a registry of claude-code's 6 named themes
//! (`utils/theme.ts`): dark / light / light-daltonized / dark-daltonized /
//! light-ansi / dark-ansi. [`ThemeName`] is one of the 6 renderable themes;
//! [`ThemeSetting`] is the stored *preference* (`auto` + the 6 names). The
//! original `TuiTheme` consts remain as a thin shim delegating to
//! [`Theme::dark`] so any straggler M6 call site compiles unchanged.

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

/// One of claude-code's 6 renderable themes (`utils/theme.ts` `THEME_NAMES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    /// Dark theme (`darkTheme`).
    Dark,
    /// Light theme (`lightTheme`).
    Light,
    /// Light colorblind-friendly theme (`lightDaltonizedTheme`).
    LightDaltonized,
    /// Dark colorblind-friendly theme (`darkDaltonizedTheme`).
    DarkDaltonized,
    /// Light ANSI-only theme (`lightAnsiTheme`).
    LightAnsi,
    /// Dark ANSI-only theme (`darkAnsiTheme`).
    DarkAnsi,
}

impl Default for ThemeName {
    /// Headless default: dark (matches `ThemeSetting::Auto.resolve()`).
    fn default() -> Self {
        ThemeName::Dark
    }
}

impl ThemeName {
    /// All renderable theme names, in `THEME_NAMES` order.
    pub const ALL: [ThemeName; 6] = [
        ThemeName::Dark,
        ThemeName::Light,
        ThemeName::LightDaltonized,
        ThemeName::DarkDaltonized,
        ThemeName::LightAnsi,
        ThemeName::DarkAnsi,
    ];

    /// Wire string, byte-for-byte with claude-code's `ThemeName`.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            ThemeName::Dark => "dark",
            ThemeName::Light => "light",
            ThemeName::LightDaltonized => "light-daltonized",
            ThemeName::DarkDaltonized => "dark-daltonized",
            ThemeName::LightAnsi => "light-ansi",
            ThemeName::DarkAnsi => "dark-ansi",
        }
    }

    /// Parse a wire string back to a `ThemeName`. `None` for unknown values.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<ThemeName> {
        ThemeName::ALL.into_iter().find(|n| n.as_wire() == s)
    }
}

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
    let bg = colorfgbg?.split(';').last()?;
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

/// Map a claude-code `ansi:<name>` color string to a neutral `StyleColor`.
/// "Bright" → the `Bright*` named variant; non-bright → the plain named
/// variant. Used only by the two `-ansi` themes. (The `tui`-side
/// `StyleColorIocraftExt` maps these to the same crossterm colors M6 used.)
#[must_use]
pub fn ansi(name: &str) -> StyleColor {
    match name {
        "ansi:black" => StyleColor::Named(NamedColor::Black),
        "ansi:blackBright" => StyleColor::Named(NamedColor::BrightBlack),
        "ansi:red" => StyleColor::Named(NamedColor::Red),
        "ansi:redBright" => StyleColor::Named(NamedColor::BrightRed),
        "ansi:green" => StyleColor::Named(NamedColor::Green),
        "ansi:greenBright" => StyleColor::Named(NamedColor::BrightGreen),
        "ansi:yellow" => StyleColor::Named(NamedColor::Yellow),
        "ansi:yellowBright" => StyleColor::Named(NamedColor::BrightYellow),
        "ansi:blue" => StyleColor::Named(NamedColor::Blue),
        "ansi:blueBright" => StyleColor::Named(NamedColor::BrightBlue),
        "ansi:magenta" => StyleColor::Named(NamedColor::Magenta),
        "ansi:magentaBright" => StyleColor::Named(NamedColor::BrightMagenta),
        "ansi:cyan" => StyleColor::Named(NamedColor::Cyan),
        "ansi:cyanBright" => StyleColor::Named(NamedColor::BrightCyan),
        "ansi:white" => StyleColor::Named(NamedColor::White),
        "ansi:whiteBright" => StyleColor::Named(NamedColor::BrightWhite),
        _ => StyleColor::Default,
    }
}

/// `StyleColor::Rgb` shorthand.
const fn rgb(r: u8, g: u8, b: u8) -> StyleColor {
    StyleColor::Rgb(r, g, b)
}

/// The active render palette. One neutral `StyleColor` per claude-code `Theme`
/// key the lingxi TUI consumes. All RGB/ANSI values copied from
/// `claude-code/src/utils/theme.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Default body text (`text`).
    pub text: StyleColor,
    /// Dim/inactive (`inactive`) — system hints, footnotes.
    pub dim: StyleColor,
    /// Error (`error`).
    pub error: StyleColor,
    /// Success / approved (`success`).
    pub success: StyleColor,
    /// Warning (`warning`).
    pub warning: StyleColor,
    /// Permission accent (`permission`) — picker header, dialogs.
    pub permission: StyleColor,
    /// Plan-mode accent (`planMode`).
    pub plan_mode: StyleColor,
    /// Suggestion / completion accent (`suggestion`).
    pub suggestion: StyleColor,
    /// Claude/assistant accent (`claude`) — assistant body color.
    pub claude: StyleColor,
    /// Diff added line bg/fg (`diffAdded`).
    pub diff_added: StyleColor,
    /// Diff removed line (`diffRemoved`).
    pub diff_removed: StyleColor,
    /// Diff added word-level (`diffAddedWord`).
    pub diff_added_word: StyleColor,
    /// Diff removed word-level (`diffRemovedWord`).
    pub diff_removed_word: StyleColor,
}

impl Default for Theme {
    /// Headless default palette: dark (matches `AppState`'s default).
    fn default() -> Self {
        Theme::dark()
    }
}

impl Theme {
    /// Dark theme — `theme.ts` `darkTheme`.
    #[must_use]
    pub const fn dark() -> Theme {
        Theme {
            text: rgb(255, 255, 255),
            dim: rgb(153, 153, 153), // inactive
            error: rgb(255, 107, 128),
            success: rgb(78, 186, 101),
            warning: rgb(255, 193, 7),
            permission: rgb(177, 185, 249),
            plan_mode: rgb(72, 150, 140),
            suggestion: rgb(177, 185, 249),
            claude: rgb(215, 119, 87),
            diff_added: rgb(34, 92, 43),
            diff_removed: rgb(122, 41, 54),
            diff_added_word: rgb(56, 166, 96),
            diff_removed_word: rgb(179, 89, 107),
        }
    }

    /// Light theme — `theme.ts` `lightTheme`.
    #[must_use]
    pub const fn light() -> Theme {
        Theme {
            text: rgb(0, 0, 0),
            dim: rgb(102, 102, 102), // inactive
            error: rgb(171, 43, 63),
            success: rgb(44, 122, 57),
            warning: rgb(150, 108, 30),
            permission: rgb(87, 105, 247),
            plan_mode: rgb(0, 102, 102),
            suggestion: rgb(87, 105, 247),
            claude: rgb(215, 119, 87),
            diff_added: rgb(105, 219, 124),
            diff_removed: rgb(255, 168, 180),
            diff_added_word: rgb(47, 157, 68),
            diff_removed_word: rgb(209, 69, 75),
        }
    }

    /// Dark daltonized — `theme.ts` `darkDaltonizedTheme`.
    #[must_use]
    pub const fn dark_daltonized() -> Theme {
        Theme {
            text: rgb(255, 255, 255),
            dim: rgb(153, 153, 153),
            error: rgb(255, 102, 102),
            success: rgb(51, 153, 255), // blue-for-green
            warning: rgb(255, 204, 0),
            permission: rgb(153, 204, 255),
            plan_mode: rgb(102, 153, 153),
            suggestion: rgb(153, 204, 255),
            claude: rgb(255, 153, 51),
            diff_added: rgb(0, 68, 102),
            diff_removed: rgb(102, 0, 0),
            diff_added_word: rgb(0, 119, 179),
            diff_removed_word: rgb(179, 0, 0),
        }
    }

    /// Light daltonized — `theme.ts` `lightDaltonizedTheme`.
    #[must_use]
    pub const fn light_daltonized() -> Theme {
        Theme {
            text: rgb(0, 0, 0),
            dim: rgb(102, 102, 102),
            error: rgb(204, 0, 0),
            success: rgb(0, 102, 153), // blue-for-green
            warning: rgb(255, 153, 0),
            permission: rgb(51, 102, 255),
            plan_mode: rgb(51, 102, 102),
            suggestion: rgb(51, 102, 255),
            claude: rgb(255, 153, 51),
            diff_added: rgb(153, 204, 255),
            diff_removed: rgb(255, 204, 204),
            diff_added_word: rgb(51, 102, 204),
            diff_removed_word: rgb(153, 51, 51),
        }
    }

    /// Dark ANSI — `theme.ts` `darkAnsiTheme` (named colors only).
    #[must_use]
    pub fn dark_ansi() -> Theme {
        Theme {
            text: ansi("ansi:whiteBright"),
            dim: ansi("ansi:white"), // inactive
            error: ansi("ansi:redBright"),
            success: ansi("ansi:greenBright"),
            warning: ansi("ansi:yellowBright"),
            permission: ansi("ansi:blueBright"),
            plan_mode: ansi("ansi:cyanBright"),
            suggestion: ansi("ansi:blueBright"),
            claude: ansi("ansi:redBright"),
            diff_added: ansi("ansi:green"),
            diff_removed: ansi("ansi:red"),
            diff_added_word: ansi("ansi:greenBright"),
            diff_removed_word: ansi("ansi:redBright"),
        }
    }

    /// Light ANSI — `theme.ts` `lightAnsiTheme` (named colors only).
    #[must_use]
    pub fn light_ansi() -> Theme {
        Theme {
            text: ansi("ansi:black"),
            dim: ansi("ansi:blackBright"), // inactive
            error: ansi("ansi:red"),
            success: ansi("ansi:green"),
            warning: ansi("ansi:yellow"),
            permission: ansi("ansi:blue"),
            plan_mode: ansi("ansi:cyan"),
            suggestion: ansi("ansi:blue"),
            claude: ansi("ansi:redBright"),
            diff_added: ansi("ansi:green"),
            diff_removed: ansi("ansi:red"),
            diff_added_word: ansi("ansi:greenBright"),
            diff_removed_word: ansi("ansi:redBright"),
        }
    }
}

/// Registry lookup: resolve a [`ThemeName`] to its concrete palette.
#[must_use]
pub fn theme_for(name: ThemeName) -> Theme {
    match name {
        ThemeName::Dark => Theme::dark(),
        ThemeName::Light => Theme::light(),
        ThemeName::DarkDaltonized => Theme::dark_daltonized(),
        ThemeName::LightDaltonized => Theme::light_daltonized(),
        ThemeName::DarkAnsi => Theme::dark_ansi(),
        ThemeName::LightAnsi => Theme::light_ansi(),
    }
}

/// Locked color constants for assistant / user / error / dim text.
///
/// (M7-15) Shim — migrated to [`Theme`] in M7-15. These consts now delegate to
/// [`Theme::dark`] values so any straggler M6 call site compiles unchanged.
/// Production render paths read from the active `AppState.theme` instead.
pub struct TuiTheme;

impl TuiTheme {
    /// Assistant body text color — `Theme::dark().claude`.
    pub const ASSISTANT: StyleColor = Theme::dark().claude;
    /// User prompt color — terminal default foreground (no theme key; claude
    /// renders user text uncolored).
    pub const USER: StyleColor = StyleColor::Default;
    /// Error/system-error text — `Theme::dark().error`.
    pub const ERROR: StyleColor = Theme::dark().error;
    /// Success text (resolved tool-use dot) — `Theme::dark().success`.
    pub const SUCCESS: StyleColor = Theme::dark().success;
    /// Dim text (system hints, footnotes) — `Theme::dark().dim`.
    pub const DIM: StyleColor = Theme::dark().dim;
    /// Warning text (e.g. an empty-state line) — `Theme::dark().warning`.
    pub const WARNING: StyleColor = Theme::dark().warning;
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
        assert_eq!(dark_ansi.success, StyleColor::Named(NamedColor::BrightGreen)); // ansi:greenBright
                                                     // Every theme resolves (no panic) and is Copy.
        for n in ThemeName::ALL {
            let _t: Theme = theme_for(n);
        }
    }
}
