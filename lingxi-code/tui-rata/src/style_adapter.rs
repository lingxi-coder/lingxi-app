//! `StyleColor` → `ratatui::style::Color` adapter — the `tui-rata` color
//! boundary, mirroring `tui`'s `StyleColorIocraftExt`.
//!
//! `tui_core::render::StyleColor` is backend-neutral; this is the only place
//! that maps it onto ratatui's color type, so the neutral core never learns
//! about any specific backend.

use ratatui::style::Color;
use tui_core::render::{NamedColor, StyleColor};

/// Map a neutral [`StyleColor`] to a ratatui [`Color`]. Named colors use
/// ratatui's native standard/`Light*` variants; indexed + rgb pass through to
/// ratatui's native `Indexed`/`Rgb` (ratatui renders both directly).
#[must_use]
pub fn to_ratatui(color: StyleColor) -> Color {
    match color {
        StyleColor::Default => Color::Reset,
        StyleColor::Named(n) => named_to_ratatui(n),
        StyleColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        StyleColor::Indexed(i) => Color::Indexed(i),
    }
}

fn named_to_ratatui(n: NamedColor) -> Color {
    match n {
        NamedColor::Black => Color::Black,
        NamedColor::Red => Color::Red,
        NamedColor::Green => Color::Green,
        NamedColor::Yellow => Color::Yellow,
        NamedColor::Blue => Color::Blue,
        NamedColor::Magenta => Color::Magenta,
        NamedColor::Cyan => Color::Cyan,
        NamedColor::White => Color::Gray,
        NamedColor::BrightBlack => Color::DarkGray,
        NamedColor::BrightRed => Color::LightRed,
        NamedColor::BrightGreen => Color::LightGreen,
        NamedColor::BrightYellow => Color::LightYellow,
        NamedColor::BrightBlue => Color::LightBlue,
        NamedColor::BrightMagenta => Color::LightMagenta,
        NamedColor::BrightCyan => Color::LightCyan,
        NamedColor::BrightWhite => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_maps_to_reset() {
        assert_eq!(to_ratatui(StyleColor::Default), Color::Reset);
    }

    #[test]
    fn rgb_and_indexed_pass_through() {
        assert_eq!(
            to_ratatui(StyleColor::Rgb(10, 20, 30)),
            Color::Rgb(10, 20, 30)
        );
        assert_eq!(to_ratatui(StyleColor::Indexed(196)), Color::Indexed(196));
    }

    #[test]
    fn named_bright_maps_to_light_variants() {
        assert_eq!(
            to_ratatui(StyleColor::Named(NamedColor::BrightRed)),
            Color::LightRed
        );
        assert_eq!(to_ratatui(StyleColor::Named(NamedColor::Red)), Color::Red);
    }
}
