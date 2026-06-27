//! Terminal background + color-depth detection feeding `ThemeSetting::Auto`.
//! One-shot OSC-11 pre-flight at startup; pure helpers below are I/O-free.

use crate::theme::ThemeName;
use std::sync::OnceLock;

/// Process-global detected background (set once at startup). `Some(Light|Dark)`
/// when the OSC-11 query succeeded; `None`/unset otherwise.
static DETECTED_BACKGROUND: OnceLock<Option<ThemeName>> = OnceLock::new();

/// Record the startup OSC-11 detection result (idempotent; first write wins).
pub(crate) fn set_detected_background(bg: Option<ThemeName>) {
    let _ = DETECTED_BACKGROUND.set(bg);
}

/// The detected background (`Light`/`Dark`), or `None` if detection didn't run
/// or didn't resolve.
pub(crate) fn detected_background() -> Option<ThemeName> {
    DETECTED_BACKGROUND.get().copied().flatten()
}

/// Parse an OSC-11 background reply body (`...rgb:RRRR/GGGG/BBBB...`) into
/// 0..1-normalized channels. Tolerates 8- or 16-bit-per-channel hex and a
/// trailing terminator (BEL `\x07` or ST `\x1b\\`). `None` if not parseable.
pub(crate) fn parse_osc11_rgb(reply: &str) -> Option<(f64, f64, f64)> {
    let body = &reply[reply.find("rgb:")? + 4..];
    let mut parts = body.split('/');
    let r = parse_channel(parts.next()?)?;
    let g = parse_channel(parts.next()?)?;
    let b = parse_channel(parts.next()?)?;
    Some((r, g, b))
}

/// Parse one hex channel (1..=4 hex digits), trimming any trailing
/// non-hex bytes (the OSC terminator). `None` if there is no leading hex.
fn parse_channel(s: &str) -> Option<f64> {
    let hex: String = s.trim().chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    if hex.is_empty() {
        return None;
    }
    let max = ((1u64 << (4 * hex.len() as u64)) - 1) as f64;
    let v = u64::from_str_radix(&hex, 16).ok()? as f64;
    Some(v / max)
}

/// ITU-R BT.709 relative luminance; `> 0.5` ⇒ a light background.
/// (Matches claude-code's `themeFromOscColor`.)
pub(crate) fn luminance_is_light(r: f64, g: f64, b: f64) -> bool {
    0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_16bit_white_and_black() {
        let (r, g, b) = parse_osc11_rgb("\x1b]11;rgb:ffff/ffff/ffff\x07").unwrap();
        assert!((r - 1.0).abs() < 1e-9 && (g - 1.0).abs() < 1e-9 && (b - 1.0).abs() < 1e-9);
        let (r, g, b) = parse_osc11_rgb("\x1b]11;rgb:0000/0000/0000\x1b\\").unwrap();
        assert_eq!((r, g, b), (0.0, 0.0, 0.0));
    }

    #[test]
    fn parses_8bit_channels() {
        let (r, g, b) = parse_osc11_rgb("rgb:ff/80/00").unwrap();
        assert!((r - 1.0).abs() < 1e-9);
        assert!((g - 128.0 / 255.0).abs() < 1e-9);
        assert_eq!(b, 0.0);
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_osc11_rgb("garbage").is_none());
        assert!(parse_osc11_rgb("rgb:zz/00/00").is_none());
        assert!(parse_osc11_rgb("rgb:ffff/ffff").is_none());
    }

    #[test]
    fn luminance_threshold() {
        assert!(luminance_is_light(1.0, 1.0, 1.0)); // white
        assert!(!luminance_is_light(0.0, 0.0, 0.0)); // black
        assert!(luminance_is_light(0.8, 0.8, 0.8)); // light grey
        assert!(!luminance_is_light(0.2, 0.2, 0.2)); // dark grey
    }
}
