//! Codex `style.rs` port (subset): the user-message / composer background
//! blend. Source: `codex-rs/tui/src/style.rs` (`user_message_bg`,
//! `user_message_style_for`, `user_message_style`) + `codex-rs/tui/src/color.rs`
//! (`blend`, `is_light`). Only the pieces `tui-rata`'s composer needs are
//! ported — the codex ANSI-256/terminal-palette fallback (`best_color`) is out
//! of scope here; the composer always renders a direct truecolor RGB.

use ratatui::style::{Color, Style};

/// Linear blend of `top` over `bottom` at `alpha` (codex `color.rs::blend`).
/// Matches codex's truncating `as u8` cast exactly (no rounding).
fn blend(top: (u8, u8, u8), bottom: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let mix = |t: u8, b: u8| ((f32::from(t) * alpha) + (f32::from(b) * (1.0 - alpha))) as u8;
    (mix(top.0, bottom.0), mix(top.1, bottom.1), mix(top.2, bottom.2))
}

/// BT.601 luma > 128 ⇒ light (codex `color.rs::is_light`).
fn is_light((r, g, b): (u8, u8, u8)) -> bool {
    (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)) > 128.0
}

/// Composer/user-message background over the detected terminal bg (codex
/// `style.rs::user_message_bg`): 4% black blended in on a light background,
/// 12% white blended in on a dark background.
pub(crate) fn user_message_bg(terminal_bg: (u8, u8, u8)) -> Color {
    let (top, alpha) = if is_light(terminal_bg) {
        ((0, 0, 0), 0.04)
    } else {
        ((255, 255, 255), 0.12)
    };
    let (r, g, b) = blend(top, terminal_bg, alpha);
    Color::Rgb(r, g, b)
}

/// Style for a user-authored area given a detected terminal background (codex
/// `style.rs::user_message_style_for`). `None` (detection didn't run or the
/// terminal didn't answer) means no background override.
pub(crate) fn user_message_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default().bg(user_message_bg(bg)),
        None => Style::default(),
    }
}

/// Style for the composer background, fed by the live OSC-11 detection
/// (codex `style.rs::user_message_style`).
pub(crate) fn user_message_style() -> Style {
    user_message_style_for(tui_core::theme_detect::detected_background_rgb())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_bg_blends_toward_white_on_dark_and_black_on_light() {
        // dark bg (30,30,30): blend 12% white → each channel
        // (255*0.12 + 30*0.88) = 57.0, truncated (codex `as u8`, no rounding) = 57.
        assert_eq!(user_message_bg((30, 30, 30)), Color::Rgb(57, 57, 57));
        // light bg (255,255,255): blend 4% black → (0*0.04 + 255*0.96) = 244.8,
        // truncated (codex `as u8`, no rounding) = 244.
        assert_eq!(user_message_bg((255, 255, 255)), Color::Rgb(244, 244, 244));
    }

    #[test]
    fn no_detected_bg_means_no_style() {
        assert_eq!(user_message_style_for(None), Style::default());
    }

    #[test]
    fn detected_bg_produces_backgrounded_style() {
        assert_eq!(
            user_message_style_for(Some((30, 30, 30))),
            Style::default().bg(Color::Rgb(57, 57, 57))
        );
    }
}
