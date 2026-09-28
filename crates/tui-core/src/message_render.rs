//! Backend-neutral terminal-line render model.
//!
//! The `TerminalSpan`/`TerminalLine` value types the message renderers produce,
//! plus the ANSI encoder used by the native-scrollback commit path. Moved from
//! `tui`'s `components::messages` during the iocraft → ratatui migration so
//! `tui-rata` can render messages without depending on iocraft. The per-variant
//! `render_entry_to_terminal_lines` dispatcher stays in `tui` (it calls the
//! iocraft component modules) and consumes these types via re-export.

use crate::render::{xterm256_to_rgb, NamedColor, StyleColor, StyledLine, StyledSpan};

/// A styled text run for native terminal scrollback output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSpan {
    /// Text payload. Must not contain a newline.
    pub text: String,
    /// Optional foreground color.
    pub fg: Option<StyleColor>,
    /// Optional background color.
    pub bg: Option<StyleColor>,
    /// Bold attribute.
    pub bold: bool,
    /// Italic attribute.
    pub italic: bool,
    /// Underline attribute.
    pub underline: bool,
}

impl TerminalSpan {
    /// A plain (unstyled) span.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }

    /// A foreground-colored span (`StyleColor::Default` becomes no color).
    #[must_use]
    pub fn colored(text: impl Into<String>, fg: StyleColor) -> Self {
        Self {
            text: text.into(),
            fg: terminal_color(fg),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }

    /// Convert a neutral [`StyledSpan`] into a `TerminalSpan`.
    #[must_use]
    pub fn from_styled_span(span: StyledSpan) -> Self {
        Self {
            text: span.text,
            fg: terminal_color(span.style.fg),
            bg: terminal_color(span.style.bg),
            bold: span.style.bold,
            italic: span.style.italic,
            underline: span.style.underline,
        }
    }
}

/// One terminal output line, split into styled spans.
pub type TerminalLine = Vec<TerminalSpan>;

fn terminal_color(color: StyleColor) -> Option<StyleColor> {
    if color == StyleColor::Default {
        None
    } else {
        Some(color)
    }
}

/// Split plain text into `TerminalLine`s (one per `\n`), unstyled.
#[must_use]
pub fn plain_terminal_lines(text: &str) -> Vec<TerminalLine> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                Vec::new()
            } else {
                vec![TerminalSpan::plain(line)]
            }
        })
        .collect()
}

/// Split text into `TerminalLine`s (one per `\n`), each rendered in `color`.
#[must_use]
pub fn colored_terminal_lines(text: &str, color: StyleColor) -> Vec<TerminalLine> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                Vec::new()
            } else {
                vec![TerminalSpan::colored(line, color)]
            }
        })
        .collect()
}

/// Convert neutral [`StyledLine`]s into `TerminalLine`s.
#[must_use]
pub fn styled_lines_to_terminal_lines(lines: Vec<StyledLine>) -> Vec<TerminalLine> {
    lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(TerminalSpan::from_styled_span)
                .collect()
        })
        .collect()
}

/// Encode one styled terminal line as ANSI bytes. The caller is responsible for
/// emitting the line terminator separately so raw-mode LF never stair-steps.
#[must_use]
pub fn encode_terminal_line_ansi(line: &TerminalLine) -> String {
    let mut out = String::new();
    for span in line {
        if span.text.is_empty() {
            continue;
        }
        let styled =
            span.fg.is_some() || span.bg.is_some() || span.bold || span.italic || span.underline;
        if styled {
            let mut codes = Vec::new();
            if let Some(fg) = span.fg {
                codes.push(fg_sgr(fg));
            }
            if let Some(bg) = span.bg {
                codes.push(bg_sgr(bg));
            }
            if span.bold {
                codes.push("1".to_string());
            }
            if span.italic {
                codes.push("3".to_string());
            }
            if span.underline {
                codes.push("4".to_string());
            }
            out.push_str("\u{1b}[");
            out.push_str(&codes.join(";"));
            out.push('m');
        }
        out.push_str(&span.text);
        if styled {
            out.push_str("\u{1b}[0m");
        }
    }
    out
}

fn fg_sgr(color: StyleColor) -> String {
    match color {
        StyleColor::Default => "39".to_string(),
        StyleColor::Named(n) => named_fg_sgr(n).to_string(),
        StyleColor::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        StyleColor::Indexed(i) => {
            let (r, g, b) = xterm256_to_rgb(i);
            format!("38;2;{r};{g};{b}")
        }
    }
}

fn named_fg_sgr(n: NamedColor) -> &'static str {
    match n {
        NamedColor::Black => "30",
        NamedColor::Red => "31",
        NamedColor::Green => "32",
        NamedColor::Yellow => "33",
        NamedColor::Blue => "34",
        NamedColor::Magenta => "35",
        NamedColor::Cyan => "36",
        NamedColor::White => "37",
        NamedColor::BrightBlack => "90",
        NamedColor::BrightRed => "91",
        NamedColor::BrightGreen => "92",
        NamedColor::BrightYellow => "93",
        NamedColor::BrightBlue => "94",
        NamedColor::BrightMagenta => "95",
        NamedColor::BrightCyan => "96",
        NamedColor::BrightWhite => "97",
    }
}

fn bg_sgr(color: StyleColor) -> String {
    match color {
        StyleColor::Default => "49".to_string(),
        StyleColor::Named(n) => named_bg_sgr(n).to_string(),
        StyleColor::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
        StyleColor::Indexed(i) => {
            let (r, g, b) = xterm256_to_rgb(i);
            format!("48;2;{r};{g};{b}")
        }
    }
}

fn named_bg_sgr(n: NamedColor) -> &'static str {
    match n {
        NamedColor::Black => "40",
        NamedColor::Red => "41",
        NamedColor::Green => "42",
        NamedColor::Yellow => "43",
        NamedColor::Blue => "44",
        NamedColor::Magenta => "45",
        NamedColor::Cyan => "46",
        NamedColor::White => "47",
        NamedColor::BrightBlack => "100",
        NamedColor::BrightRed => "101",
        NamedColor::BrightGreen => "102",
        NamedColor::BrightYellow => "103",
        NamedColor::BrightBlue => "104",
        NamedColor::BrightMagenta => "105",
        NamedColor::BrightCyan => "106",
        NamedColor::BrightWhite => "107",
    }
}
