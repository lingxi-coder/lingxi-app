//! TUI color palette (M7-15 theme picker).
//!
//! M6-02 shipped four fixed `TuiTheme` consts. M7-15 expands this into a real
//! [`Theme`] value-struct + a registry of claude-code's 6 named themes
//! (`utils/theme.ts`): dark / light / light-daltonized / dark-daltonized /
//! light-ansi / dark-ansi. [`ThemeName`] is one of the 6 renderable themes;
//! [`ThemeSetting`] is the stored *preference* (`auto` + the 6 names). The
//! original `TuiTheme` consts remain as a thin shim delegating to
//! [`Theme::dark`] so any straggler M6 call site compiles unchanged.

use iocraft::Color;

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

    /// Resolve to a concrete renderable theme. Headless TUI cannot probe the
    /// terminal background, so `Auto` resolves to `Dark` (claude-code's
    /// dark-default fallback). Real terminal-bg detection is deferred to M8.
    #[must_use]
    pub const fn resolve(self) -> ThemeName {
        match self {
            ThemeSetting::Auto => ThemeName::Dark,
            ThemeSetting::Named(n) => n,
        }
    }
}

/// Map a claude-code `ansi:<name>` color string to an iocraft `Color`.
/// "Bright" → the un-prefixed crossterm variant; non-bright → the `Dark*`
/// variant. Used only by the two `-ansi` themes.
#[must_use]
pub fn ansi(name: &str) -> Color {
    match name {
        "ansi:black" => Color::Black,
        "ansi:blackBright" => Color::DarkGrey,
        "ansi:red" => Color::DarkRed,
        "ansi:redBright" => Color::Red,
        "ansi:green" => Color::DarkGreen,
        "ansi:greenBright" => Color::Green,
        "ansi:yellow" => Color::DarkYellow,
        "ansi:yellowBright" => Color::Yellow,
        "ansi:blue" => Color::DarkBlue,
        "ansi:blueBright" => Color::Blue,
        "ansi:magenta" => Color::DarkMagenta,
        "ansi:magentaBright" => Color::Magenta,
        "ansi:cyan" => Color::DarkCyan,
        "ansi:cyanBright" => Color::Cyan,
        "ansi:white" => Color::Grey,
        "ansi:whiteBright" => Color::White,
        _ => Color::Reset,
    }
}

/// `Color::Rgb` shorthand.
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb { r, g, b }
}

/// The active render palette. One iocraft `Color` per claude-code `Theme`
/// key the lingxi TUI consumes. All RGB/ANSI values copied from
/// `claude-code/src/utils/theme.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Default body text (`text`).
    pub text: Color,
    /// Dim/inactive (`inactive`) — system hints, footnotes.
    pub dim: Color,
    /// Error (`error`).
    pub error: Color,
    /// Success / approved (`success`).
    pub success: Color,
    /// Warning (`warning`).
    pub warning: Color,
    /// Permission accent (`permission`) — picker header, dialogs.
    pub permission: Color,
    /// Plan-mode accent (`planMode`).
    pub plan_mode: Color,
    /// Suggestion / completion accent (`suggestion`).
    pub suggestion: Color,
    /// Claude/assistant accent (`claude`) — assistant body color.
    pub claude: Color,
    /// Diff added line bg/fg (`diffAdded`).
    pub diff_added: Color,
    /// Diff removed line (`diffRemoved`).
    pub diff_removed: Color,
    /// Diff added word-level (`diffAddedWord`).
    pub diff_added_word: Color,
    /// Diff removed word-level (`diffRemovedWord`).
    pub diff_removed_word: Color,
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
    pub const ASSISTANT: Color = Theme::dark().claude;
    /// User prompt color — terminal default foreground (no theme key; claude
    /// renders user text uncolored).
    pub const USER: Color = Color::Reset;
    /// Error/system-error text — `Theme::dark().error`.
    pub const ERROR: Color = Theme::dark().error;
    /// Dim text (system hints, footnotes) — `Theme::dark().dim`.
    pub const DIM: Color = Theme::dark().dim;
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
        assert!(matches!(TuiTheme::USER, Color::Reset));
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
        assert!(matches!(ansi("ansi:red"), Color::DarkRed));
        assert!(matches!(ansi("ansi:redBright"), Color::Red));
        assert!(matches!(ansi("ansi:white"), Color::Grey));
        assert!(matches!(ansi("ansi:whiteBright"), Color::White));
        assert!(matches!(ansi("ansi:black"), Color::Black));
        assert!(matches!(ansi("ansi:blackBright"), Color::DarkGrey));
    }

    #[test]
    fn auto_resolves_to_dark_headless() {
        assert_eq!(ThemeSetting::Auto.resolve(), ThemeName::Dark);
        assert_eq!(ThemeSetting::Named(ThemeName::Light).resolve(), ThemeName::Light);
    }

    #[test]
    fn registry_returns_distinct_palettes_with_locked_colors() {
        let dark = theme_for(ThemeName::Dark);
        let light = theme_for(ThemeName::Light);
        // dark.text = rgb(255,255,255); light.text = rgb(0,0,0)  (theme.ts).
        assert_eq!(dark.text, Color::Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(light.text, Color::Rgb { r: 0, g: 0, b: 0 });
        // dark.error = rgb(255,107,128); light.error = rgb(171,43,63).
        assert_eq!(dark.error, Color::Rgb { r: 255, g: 107, b: 128 });
        assert_eq!(light.error, Color::Rgb { r: 171, g: 43, b: 63 });
        // dark.claude = rgb(215,119,87)  (the Claude orange accent).
        assert_eq!(dark.claude, Color::Rgb { r: 215, g: 119, b: 87 });
        // ANSI theme uses named colors, not Rgb.
        let dark_ansi = theme_for(ThemeName::DarkAnsi);
        assert_eq!(dark_ansi.error, Color::Red); // ansi:redBright
        assert_eq!(dark_ansi.success, Color::Green); // ansi:greenBright
        // Every theme resolves (no panic) and is Copy.
        for n in ThemeName::ALL {
            let _t: Theme = theme_for(n);
        }
    }
}
