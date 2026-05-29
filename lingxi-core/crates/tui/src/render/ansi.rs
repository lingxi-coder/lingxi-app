//! ANSI / SGR parser producing [`StyledLine`]s.
//!
//! Moved+expanded from M6's `src/ansi.rs`. M6 handled 8/16-color SGR +
//! reset/bold only. M7-01 adds 256-color (`38;5;N` / `48;5;N`), truecolor
//! (`38;2;r;g;b` / `48;2;r;g;b`), italic/underline attributes, and safely
//! skips cursor-move / erase CSI sequences (CUU/CUD/CUF/CUB/ED/EL) without
//! corrupting surrounding text. (256/truecolor + cursor-skip land in the
//! following tasks; this file first re-establishes M6 parity.)
//!
//! Operates on `&str` (UTF-8 already decoded). Output splits on `\n` into
//! one [`StyledLine`] per visual line. Never panics on malformed input.

use crate::render::{NamedColor, SpanStyle, StyleColor, StyledLine, StyledSpan};

/// Parse `input` into styled lines. Unsupported escape sequences are
/// silently skipped. Never panics.
#[must_use]
pub fn parse_ansi(input: &str) -> Vec<StyledLine> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<StyledLine> = Vec::new();
    let mut current: Vec<StyledSpan> = Vec::new();
    let mut style = SpanStyle::default();
    let mut buf = String::new();
    let bytes = input.as_bytes();
    let mut i = 0;

    // Flush `buf` into `current` as a span with the active style.
    macro_rules! flush_buf {
        () => {
            if !buf.is_empty() {
                current.push(StyledSpan::styled(std::mem::take(&mut buf), style));
            }
        };
    }

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' {
            flush_buf!();
            lines.push(StyledLine {
                spans: coalesce(std::mem::take(&mut current)),
            });
            i += 1;
            continue;
        }
        if b == 0x1b && i + 1 < bytes.len() {
            flush_buf!();
            let next = bytes[i + 1];
            if next == b'[' {
                // CSI — read params until a final byte in 0x40..=0x7E.
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
                    // Unterminated CSI — drop it; flush what we have.
                    break;
                }
                let final_byte = bytes[j];
                if final_byte == b'm' {
                    apply_sgr(&params, &mut style);
                }
                // Else: non-SGR CSI (cursor/erase/mode) — skipped (Task 5
                // makes the skip explicit + tested).
                i = j + 1;
                continue;
            } else if next == b']' {
                // OSC — read to BEL (0x07) or ST (ESC \).
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
            // Other ESC-prefixed sequence (ESC c, ESC =, …). Skip 2 bytes.
            i += 2;
            continue;
        }
        buf.push(b as char);
        i += 1;
    }

    flush_buf!();
    if !current.is_empty() {
        lines.push(StyledLine {
            spans: coalesce(current),
        });
    }
    lines
}

/// Merge adjacent spans with identical style + kind so skipped escape
/// sequences (which split a run) collapse back into one span.
fn coalesce(spans: Vec<StyledSpan>) -> Vec<StyledSpan> {
    let mut out: Vec<StyledSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        match out.last_mut() {
            Some(prev) if prev.style == span.style && prev.kind == span.kind => {
                prev.text.push_str(&span.text);
            }
            _ => out.push(span),
        }
    }
    out
}

/// Apply a semicolon-separated SGR parameter string to `style`. Empty
/// params (`\x1b[m`) reset. Unsupported codes are ignored. (256/truecolor
/// extended forms land in Task 4.)
fn apply_sgr(params: &str, style: &mut SpanStyle) {
    if params.is_empty() {
        *style = SpanStyle::default();
        return;
    }
    for tok in params.split(';') {
        let n: u16 = tok.parse().unwrap_or(0);
        match n {
            0 => *style = SpanStyle::default(),
            1 => style.bold = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => style.bold = false,
            23 => style.italic = false,
            24 => style.underline = false,
            30 => style.fg = StyleColor::Named(NamedColor::Black),
            31 => style.fg = StyleColor::Named(NamedColor::Red),
            32 => style.fg = StyleColor::Named(NamedColor::Green),
            33 => style.fg = StyleColor::Named(NamedColor::Yellow),
            34 => style.fg = StyleColor::Named(NamedColor::Blue),
            35 => style.fg = StyleColor::Named(NamedColor::Magenta),
            36 => style.fg = StyleColor::Named(NamedColor::Cyan),
            37 => style.fg = StyleColor::Named(NamedColor::White),
            39 => style.fg = StyleColor::Default,
            40 => style.bg = StyleColor::Named(NamedColor::Black),
            41 => style.bg = StyleColor::Named(NamedColor::Red),
            42 => style.bg = StyleColor::Named(NamedColor::Green),
            43 => style.bg = StyleColor::Named(NamedColor::Yellow),
            44 => style.bg = StyleColor::Named(NamedColor::Blue),
            45 => style.bg = StyleColor::Named(NamedColor::Magenta),
            46 => style.bg = StyleColor::Named(NamedColor::Cyan),
            47 => style.bg = StyleColor::Named(NamedColor::White),
            49 => style.bg = StyleColor::Default,
            90 => style.fg = StyleColor::Named(NamedColor::BrightBlack),
            91 => style.fg = StyleColor::Named(NamedColor::BrightRed),
            92 => style.fg = StyleColor::Named(NamedColor::BrightGreen),
            93 => style.fg = StyleColor::Named(NamedColor::BrightYellow),
            94 => style.fg = StyleColor::Named(NamedColor::BrightBlue),
            95 => style.fg = StyleColor::Named(NamedColor::BrightMagenta),
            96 => style.fg = StyleColor::Named(NamedColor::BrightCyan),
            97 => style.fg = StyleColor::Named(NamedColor::BrightWhite),
            100 => style.bg = StyleColor::Named(NamedColor::BrightBlack),
            101 => style.bg = StyleColor::Named(NamedColor::BrightRed),
            102 => style.bg = StyleColor::Named(NamedColor::BrightGreen),
            103 => style.bg = StyleColor::Named(NamedColor::BrightYellow),
            104 => style.bg = StyleColor::Named(NamedColor::BrightBlue),
            105 => style.bg = StyleColor::Named(NamedColor::BrightMagenta),
            106 => style.bg = StyleColor::Named(NamedColor::BrightCyan),
            107 => style.bg = StyleColor::Named(NamedColor::BrightWhite),
            _ => { /* unsupported / extended (38/48) — handled in Task 4 */ }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{NamedColor, SpanKind, StyleColor};

    fn first_line(input: &str) -> StyledLine {
        parse_ansi(input).into_iter().next().unwrap_or_default()
    }

    #[test]
    fn red_err_then_reset() {
        let line = first_line("\x1b[31mERR\x1b[0m");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "ERR");
        assert_eq!(line.spans[0].style.fg, StyleColor::Named(NamedColor::Red));
    }

    #[test]
    fn multi_param_sgr_bold_red() {
        let line = first_line("\x1b[1;31mhi\x1b[0m");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "hi");
        assert_eq!(line.spans[0].style.fg, StyleColor::Named(NamedColor::Red));
        assert!(line.spans[0].style.bold);
    }

    #[test]
    fn empty_string_parses_to_empty_vec() {
        assert!(parse_ansi("").is_empty());
    }

    #[test]
    fn newline_splits_into_lines() {
        let lines = parse_ansi("a\nb");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].plain_text(), "a");
        assert_eq!(lines[1].plain_text(), "b");
    }

    #[test]
    fn all_spans_are_text_kind() {
        let line = first_line("\x1b[31mERR\x1b[0m");
        assert_eq!(line.spans[0].kind, SpanKind::Text);
    }

    #[test]
    fn malformed_unterminated_csi_does_not_panic() {
        let _ = parse_ansi("\x1b[31");
    }
}
