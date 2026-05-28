//! Minimal ANSI SGR parser for Bash tool output.
//!
//! Scope (M6-04 — full parser deferred to M7):
//!   - CSI `\x1b[Nm` where N is in {0 (reset), 1 (bold), 22 (bold off),
//!     30..=37 (fg), 39 (fg default), 40..=47 (bg), 49 (bg default),
//!     90..=97 (bright fg), 100..=107 (bright bg)}.
//!   - Multi-parameter forms `\x1b[1;31m` (bold + red) — semicolon-separated.
//!   - Empty SGR `\x1b[m` is treated as reset (per spec).
//!   - ALL other CSI / OSC / DCS / SOS / PM / APC sequences are SKIPPED
//!     (text between `\x1b[` / `\x1b]` and the terminator is dropped; the
//!     terminator itself is dropped).
//!   - Malformed input (unterminated CSI, lone ESC, junk bytes) does NOT
//!     panic — unterminated sequences are consumed up to EOF and ignored.
//!
//! Operates on `&str` (the caller has already decoded UTF-8). Non-ASCII
//! bytes inside escape sequences are not expected; SGR parameters are
//! plain ASCII digits.

/// Foreground / background color slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnsiColor {
    /// Terminal default — no override.
    #[default]
    Default,
    /// SGR 30 / 40.
    Black,
    /// SGR 31 / 41.
    Red,
    /// SGR 32 / 42.
    Green,
    /// SGR 33 / 43.
    Yellow,
    /// SGR 34 / 44.
    Blue,
    /// SGR 35 / 45.
    Magenta,
    /// SGR 36 / 46.
    Cyan,
    /// SGR 37 / 47.
    White,
    /// SGR 90 / 100.
    BrightBlack,
    /// SGR 91 / 101.
    BrightRed,
    /// SGR 92 / 102.
    BrightGreen,
    /// SGR 93 / 103.
    BrightYellow,
    /// SGR 94 / 104.
    BrightBlue,
    /// SGR 95 / 105.
    BrightMagenta,
    /// SGR 96 / 106.
    BrightCyan,
    /// SGR 97 / 107.
    BrightWhite,
}

/// Active style at a given parser position. Reset by `\x1b[0m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnsiStyle {
    /// Foreground color.
    pub fg: AnsiColor,
    /// Background color.
    pub bg: AnsiColor,
    /// SGR 1 (set on) / SGR 22 (set off).
    pub bold: bool,
}

/// One styled run produced by [`parse_ansi`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsiSpan {
    /// Style active for this run.
    pub style: AnsiStyle,
    /// UTF-8 text content of the run.
    pub text: String,
}

/// Parse `input` into a sequence of styled spans. Unsupported escape
/// sequences are silently skipped. Never panics.
#[must_use]
pub fn parse_ansi(input: &str) -> Vec<AnsiSpan> {
    let mut spans: Vec<AnsiSpan> = Vec::new();
    let mut style = AnsiStyle::default();
    let mut buf = String::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b && i + 1 < bytes.len() {
            // Flush current buf as a span before processing the escape.
            if !buf.is_empty() {
                spans.push(AnsiSpan {
                    style,
                    text: std::mem::take(&mut buf),
                });
            }
            let next = bytes[i + 1];
            if next == b'[' {
                // CSI — read until a final byte in 0x40..=0x7E.
                let mut j = i + 2;
                let mut params = String::new();
                while j < bytes.len() {
                    let c = bytes[j];
                    if (0x40..=0x7E).contains(&c) {
                        break;
                    }
                    params.push(c as char);
                    j += 1;
                }
                if j >= bytes.len() {
                    // Unterminated CSI — drop it and stop.
                    return spans;
                }
                let final_byte = bytes[j];
                if final_byte == b'm' {
                    apply_sgr(&params, &mut style);
                }
                // Else: ignored CSI (cursor movement, mode set, etc).
                i = j + 1;
                continue;
            } else if next == b']' {
                // OSC — read until BEL (0x07) or ST (ESC \).
                let mut j = i + 2;
                while j < bytes.len() {
                    if bytes[j] == 0x07 {
                        j += 1;
                        break;
                    }
                    if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                        j += 2;
                        break;
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            // Other ESC-prefixed sequence (e.g. ESC c, ESC =). Skip 2 bytes.
            i += 2;
            continue;
        }
        // Push byte as-is. Multi-byte UTF-8 just falls through as individual
        // continuation bytes (the underlying `&str` is valid UTF-8 already).
        buf.push(b as char);
        i += 1;
    }
    if !buf.is_empty() {
        spans.push(AnsiSpan { style, text: buf });
    }
    spans
}

fn apply_sgr(params: &str, style: &mut AnsiStyle) {
    if params.is_empty() {
        *style = AnsiStyle::default();
        return;
    }
    for tok in params.split(';') {
        let n: u16 = tok.parse().unwrap_or(0);
        match n {
            0 => *style = AnsiStyle::default(),
            1 => style.bold = true,
            22 => style.bold = false,
            30 => style.fg = AnsiColor::Black,
            31 => style.fg = AnsiColor::Red,
            32 => style.fg = AnsiColor::Green,
            33 => style.fg = AnsiColor::Yellow,
            34 => style.fg = AnsiColor::Blue,
            35 => style.fg = AnsiColor::Magenta,
            36 => style.fg = AnsiColor::Cyan,
            37 => style.fg = AnsiColor::White,
            39 => style.fg = AnsiColor::Default,
            40 => style.bg = AnsiColor::Black,
            41 => style.bg = AnsiColor::Red,
            42 => style.bg = AnsiColor::Green,
            43 => style.bg = AnsiColor::Yellow,
            44 => style.bg = AnsiColor::Blue,
            45 => style.bg = AnsiColor::Magenta,
            46 => style.bg = AnsiColor::Cyan,
            47 => style.bg = AnsiColor::White,
            49 => style.bg = AnsiColor::Default,
            90 => style.fg = AnsiColor::BrightBlack,
            91 => style.fg = AnsiColor::BrightRed,
            92 => style.fg = AnsiColor::BrightGreen,
            93 => style.fg = AnsiColor::BrightYellow,
            94 => style.fg = AnsiColor::BrightBlue,
            95 => style.fg = AnsiColor::BrightMagenta,
            96 => style.fg = AnsiColor::BrightCyan,
            97 => style.fg = AnsiColor::BrightWhite,
            100 => style.bg = AnsiColor::BrightBlack,
            101 => style.bg = AnsiColor::BrightRed,
            102 => style.bg = AnsiColor::BrightGreen,
            103 => style.bg = AnsiColor::BrightYellow,
            104 => style.bg = AnsiColor::BrightBlue,
            105 => style.bg = AnsiColor::BrightMagenta,
            106 => style.bg = AnsiColor::BrightCyan,
            107 => style.bg = AnsiColor::BrightWhite,
            _ => { /* unsupported code — ignore */ }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_err_then_reset() {
        let v = parse_ansi("\x1b[31mERR\x1b[0m");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].text, "ERR");
        assert_eq!(v[0].style.fg, AnsiColor::Red);
    }
}
