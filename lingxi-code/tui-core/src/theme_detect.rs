//! Terminal background + color-depth detection feeding `ThemeSetting::Auto`.
//! One-shot OSC-11 pre-flight at startup; pure helpers below are I/O-free.
//! `TimedStdin` uses `rustix::event::poll` + `rustix::io::read` — fully safe.

use crate::theme::ThemeName;
use std::io::{Read, Write};
use std::sync::OnceLock;

/// Process-global detected background (set once at startup). `Some(Light|Dark)`
/// when the OSC-11 query succeeded; `None`/unset otherwise.
static DETECTED_BACKGROUND: OnceLock<Option<ThemeName>> = OnceLock::new();

/// Process-global detected background as 8-bit RGB (set once at startup,
/// alongside [`DETECTED_BACKGROUND`]). `Some((r,g,b))` when the OSC-11 query
/// succeeded and parsed; `None`/unset otherwise. Feeds `tui-rata`'s
/// `style::user_message_style()` composer-background blend.
static DETECTED_BACKGROUND_RGB: OnceLock<Option<(u8, u8, u8)>> = OnceLock::new();

/// Record the startup OSC-11 detection result (idempotent; first write wins).
pub(crate) fn set_detected_background(bg: Option<ThemeName>) {
    let _ = DETECTED_BACKGROUND.set(bg);
}

/// The detected background (`Light`/`Dark`), or `None` if detection didn't run
/// or didn't resolve.
pub(crate) fn detected_background() -> Option<ThemeName> {
    DETECTED_BACKGROUND.get().copied().flatten()
}

/// Quantize a parsed 0.0-1.0 OSC-11 triple to 8-bit channels.
pub(crate) fn quantize_rgb((r, g, b): (f64, f64, f64)) -> (u8, u8, u8) {
    let q = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    (q(r), q(g), q(b))
}

/// The OSC-11-detected terminal background as 8-bit RGB, when detection ran
/// and the terminal answered. `None` before detection or on no-reply.
pub fn detected_background_rgb() -> Option<(u8, u8, u8)> {
    DETECTED_BACKGROUND_RGB.get().copied().flatten()
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
    let hex: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
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

/// The OSC-11 background query.
const OSC11_QUERY: &[u8] = b"\x1b]11;?\x07";

/// I/O-injectable detection core: write the OSC-11 query to `writer`, then read
/// the reply from `reader` until a terminator (BEL `\x07` or ST `\x1b\\`), EOF,
/// or a 1 KiB cap, and classify it. The `reader` owns timing — a real terminal
/// reader returns `Ok(0)` on timeout (see `detect_terminal_theme`), so this
/// loop ends without a reply and returns `None`.
pub(crate) fn detect_with_io<R: Read, W: Write>(mut reader: R, mut writer: W) -> Option<ThemeName> {
    writer.write_all(OSC11_QUERY).ok()?;
    writer.flush().ok()?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                let terminated = buf.contains(&0x07) || buf.windows(2).any(|w| w == [0x1b, 0x5c]);
                if terminated || buf.len() > 1024 {
                    break;
                }
            }
        }
    }
    match parse_osc11_rgb(&String::from_utf8_lossy(&buf)) {
        Some((r, g, b)) => {
            let _ = DETECTED_BACKGROUND_RGB.set(Some(quantize_rgb((r, g, b))));
            Some(if luminance_is_light(r, g, b) {
                ThemeName::Light
            } else {
                ThemeName::Dark
            })
        }
        None => {
            let _ = DETECTED_BACKGROUND_RGB.set(None);
            None
        }
    }
}

/// A `Read` over stdin that returns `Ok(0)` once a deadline passes, so a silent
/// terminal can't make detection hang. Uses `poll(2)` so there is NO background
/// reader thread that could steal the user's first keystroke.
#[cfg(unix)]
struct TimedStdin {
    deadline: std::time::Instant,
}

#[cfg(unix)]
impl Read for TimedStdin {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::fd::AsFd;
        let now = std::time::Instant::now();
        if now >= self.deadline {
            return Ok(0);
        }
        let ms = (self.deadline - now).as_millis().min(i32::MAX as u128) as i32;
        let stdin = std::io::stdin();
        let fd = stdin.as_fd();
        let mut fds = [rustix::event::PollFd::new(
            &fd,
            rustix::event::PollFlags::IN,
        )];
        match rustix::event::poll(&mut fds, ms) {
            Ok(0) | Err(_) => return Ok(0), // timeout or poll error → behave like EOF
            Ok(_) => {}
        }
        rustix::io::read(&fd, buf).map_err(std::io::Error::from)
    }
}

/// One-shot startup pre-flight: query the terminal background via OSC-11 and
/// cache the result for `ThemeSetting::Auto`. Best-effort and non-blocking
/// (≤ ~100 ms). No-op unless both stdin and stdout are TTYs. Unix-only; on other
/// platforms Auto falls back to `$COLORFGBG`/Dark.
pub fn detect_terminal_theme() {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return;
        }
        if crossterm::terminal::enable_raw_mode().is_err() {
            return;
        }
        let reader = TimedStdin {
            deadline: std::time::Instant::now() + std::time::Duration::from_millis(100),
        };
        let bg = detect_with_io(reader, std::io::stdout());
        let _ = crossterm::terminal::disable_raw_mode();
        set_detected_background(bg);
    }
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

    #[test]
    fn detect_with_io_light_reply_and_sends_query() {
        let reply = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
        let mut sent = Vec::new();
        let got = detect_with_io(std::io::Cursor::new(&reply[..]), &mut sent);
        assert_eq!(got, Some(ThemeName::Light));
        assert_eq!(sent, b"\x1b]11;?\x07"); // it sent the OSC-11 query
    }

    #[test]
    fn detect_with_io_dark_reply() {
        let reply = b"\x1b]11;rgb:0000/0000/0000\x07";
        let got = detect_with_io(std::io::Cursor::new(&reply[..]), std::io::sink());
        assert_eq!(got, Some(ThemeName::Dark));
    }

    #[test]
    fn detect_with_io_no_reply_is_none() {
        let got = detect_with_io(std::io::Cursor::new(&b""[..]), std::io::sink());
        assert_eq!(got, None);
    }

    #[test]
    fn detected_rgb_is_stored_alongside_theme() {
        // The OnceLock is process-global; exercise the parse+quantize helper the
        // setter uses instead of the global itself.
        assert_eq!(quantize_rgb((1.0, 1.0, 1.0)), (255, 255, 255));
        assert_eq!(quantize_rgb((0.0, 0.5, 1.0)), (0, 128, 255));
    }
}
