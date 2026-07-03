//! Render `tui_core`'s neutral styled-line model into ratatui text.
//!
//! `tui_core::render::StyledLine`/`StyledSpan` are backend-neutral; this module
//! is the `tui` side that maps them onto `ratatui::text::Line`/`Span`,
//! using [`crate::style_adapter`] for the color boundary. This is the seam that
//! lets `tui` display everything `tui-core` renders (markdown, ANSI,
//! diffs, message bodies) without the neutral core knowing about ratatui.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tui_core::render::{StyleColor, StyledLine, StyledSpan};

use crate::style_adapter::to_ratatui;

/// Convert one neutral [`StyledLine`] into a ratatui [`Line`].
#[must_use]
pub fn styled_line_to_ratatui(line: &StyledLine) -> Line<'static> {
    Line::from(
        line.spans
            .iter()
            .map(styled_span_to_ratatui)
            .collect::<Vec<_>>(),
    )
}

/// Convert one neutral [`StyledSpan`] into a ratatui [`Span`]. `Default` colors
/// are left unset so the terminal's own defaults show through.
#[must_use]
pub fn styled_span_to_ratatui(span: &StyledSpan) -> Span<'static> {
    let mut style = Style::default();
    if span.style.fg != StyleColor::Default {
        style = style.fg(to_ratatui(span.style.fg));
    }
    if span.style.bg != StyleColor::Default {
        style = style.bg(to_ratatui(span.style.bg));
    }
    if span.style.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.style.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.style.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    Span::styled(span.text.clone(), style)
}

/// Convert one message terminal line into a ratatui [`Line`].
#[must_use]
pub fn terminal_line_to_ratatui(line: &tui_core::message_render::TerminalLine) -> Line<'static> {
    Line::from(
        line.iter()
            .map(|span| Span::styled(span.text.clone(), terminal_span_style(span)))
            .collect::<Vec<_>>(),
    )
}

fn terminal_span_style(span: &tui_core::message_render::TerminalSpan) -> Style {
    let mut style = Style::default();
    if let Some(fg) = span.fg {
        style = style.fg(to_ratatui(fg));
    }
    if let Some(bg) = span.bg {
        style = style.bg(to_ratatui(bg));
    }
    if span.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;
    use tui_core::render::{NamedColor, SpanStyle};

    #[test]
    fn plain_span_carries_no_style() {
        let span = styled_span_to_ratatui(&StyledSpan::plain("hi"));
        assert_eq!(span.content, "hi");
        assert_eq!(span.style, Style::default());
    }

    #[test]
    fn styled_span_maps_color_and_modifiers() {
        let span = styled_span_to_ratatui(&StyledSpan::styled(
            "x",
            SpanStyle {
                fg: StyleColor::Named(NamedColor::BrightGreen),
                bold: true,
                underline: true,
                ..SpanStyle::default()
            },
        ));
        assert_eq!(span.style.fg, Some(Color::LightGreen));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn line_preserves_span_order_and_text() {
        let line = StyledLine {
            spans: vec![StyledSpan::plain("a"), StyledSpan::plain("b")],
        };
        let out = styled_line_to_ratatui(&line);
        let text: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "ab");
    }

    #[test]
    fn terminal_line_converts_to_ratatui_line() {
        use tui_core::message_render::{TerminalLine, TerminalSpan};
        let line: TerminalLine = vec![
            TerminalSpan::plain("hello "),
            TerminalSpan::colored("world", StyleColor::Named(NamedColor::BrightGreen)),
        ];
        let out = terminal_line_to_ratatui(&line);
        let text: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "hello world");
        assert_eq!(out.spans[1].style.fg, Some(Color::LightGreen));
    }
}
