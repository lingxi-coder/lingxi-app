//! iocraft adapter for the neutral [`StyleColor`] render model.
//!
//! `StyleColor` (in [`crate::render`]) is backend-neutral so it can live in
//! `tui-core`. The mapping to `iocraft::Color` — which cannot move to the
//! neutral crate — lives here as an extension trait, keeping the
//! `.to_iocraft()` call syntax the iocraft render paths already use.

use iocraft::Color;

use tui_core::render::{xterm256_to_rgb, NamedColor, StyleColor};

/// Extension mapping a neutral [`StyleColor`] to an iocraft [`Color`].
pub trait StyleColorIocraftExt {
    /// Map to an iocraft [`Color`]. Named colors map to crossterm's "dark"
    /// range for standard and the non-dark range for bright, matching M6's
    /// `ansi_to_iocraft_color`. Indexed colors resolve through the xterm
    /// 256-color cube to an `Rgb` triple. `Rgb` passes through.
    fn to_iocraft(self) -> Color;
}

impl StyleColorIocraftExt for StyleColor {
    fn to_iocraft(self) -> Color {
        match self {
            StyleColor::Default => Color::Reset,
            StyleColor::Named(n) => named_to_iocraft(n),
            StyleColor::Rgb(r, g, b) => Color::Rgb { r, g, b },
            StyleColor::Indexed(i) => {
                let (r, g, b) = xterm256_to_rgb(i);
                Color::Rgb { r, g, b }
            }
        }
    }
}

fn named_to_iocraft(n: NamedColor) -> Color {
    match n {
        NamedColor::Black => Color::Black,
        NamedColor::Red => Color::DarkRed,
        NamedColor::Green => Color::DarkGreen,
        NamedColor::Yellow => Color::DarkYellow,
        NamedColor::Blue => Color::DarkBlue,
        NamedColor::Magenta => Color::DarkMagenta,
        NamedColor::Cyan => Color::DarkCyan,
        NamedColor::White => Color::Grey,
        NamedColor::BrightBlack => Color::DarkGrey,
        NamedColor::BrightRed => Color::Red,
        NamedColor::BrightGreen => Color::Green,
        NamedColor::BrightYellow => Color::Yellow,
        NamedColor::BrightBlue => Color::Blue,
        NamedColor::BrightMagenta => Color::Magenta,
        NamedColor::BrightCyan => Color::Cyan,
        NamedColor::BrightWhite => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_color_maps_to_iocraft() {
        assert!(matches!(StyleColor::Default.to_iocraft(), Color::Reset));
        assert!(matches!(
            StyleColor::Named(NamedColor::Red).to_iocraft(),
            Color::DarkRed
        ));
        assert!(matches!(
            StyleColor::Named(NamedColor::BrightRed).to_iocraft(),
            Color::Red
        ));
        assert!(matches!(
            StyleColor::Rgb(10, 20, 30).to_iocraft(),
            Color::Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        ));
        // 256-palette index resolves to an Rgb triple via the xterm cube.
        assert!(matches!(
            StyleColor::Indexed(196).to_iocraft(),
            Color::Rgb { .. }
        ));
    }
}
