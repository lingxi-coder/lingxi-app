//! Render `tui_core`'s neutral styled-line model into ratatui text.
//!
//! `tui_core::render::StyledLine`/`StyledSpan` are backend-neutral; this module
//! is the `tui` side that maps them onto `ratatui::text::Line`/`Span`,
//! using [`crate::style_adapter`] for the color boundary. This is the seam that
//! lets `tui` display everything `tui-core` renders (markdown, ANSI,
//! diffs, message bodies) without the neutral core knowing about ratatui.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::WidgetRef;
use tui_core::render::osc8::hyperlink;
use tui_core::render::{StyleColor, StyledLine, StyledSpan};
use unicode_width::UnicodeWidthChar;

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

/// Render a line containing OSC 8 metadata into ratatui's cell buffer.
///
/// Ratatui's ordinary `Line` renderer treats the URL inside an OSC sequence
/// as visible text while splitting a span into graphemes.  A full-screen
/// frame therefore needs the same per-cell workaround as ratatui's own
/// hyperlink example: keep the visible grapheme in its normal cell, but wrap
/// that cell's symbol in an independent OSC 8 pair.  Independent pairs also
/// keep terminal hyperlinks correct when `Terminal::flush` redraws only an
/// interior cell of a previously rendered link.
pub(crate) fn render_line_with_hyperlinks(line: &Line<'_>, area: Rect, buf: &mut Buffer) {
    if !line
        .spans
        .iter()
        .any(|span| span.content.contains("\x1b]8;;"))
    {
        WidgetRef::render_ref(line, area, buf);
        return;
    }

    let area = area.intersection(buf.area);
    if area.is_empty() {
        return;
    }
    buf.set_style(area, line.style);

    let mut cells: Vec<BufferedHyperlinkCell> = Vec::new();
    let mut target = None;
    for span in &line.spans {
        let style = span.style.patch(line.style);
        let mut cursor = 0;
        while cursor < span.content.len() {
            if let Some((next, next_target)) = osc8_control_at(&span.content, cursor) {
                target = next_target;
                cursor = next;
                continue;
            }
            let Some(ch) = span.content[cursor..].chars().next() else {
                break;
            };
            let next = cursor + ch.len_utf8();
            let width = ch.width().unwrap_or(0);
            if width == 0 {
                if let Some(previous) = cells.last_mut() {
                    previous.text.push(ch);
                }
            } else {
                cells.push(BufferedHyperlinkCell {
                    text: ch.to_string(),
                    target: target.clone(),
                    style,
                    width,
                });
            }
            cursor = next;
        }
    }

    let line_width: usize = cells.iter().map(|cell| cell.width).sum();
    let available = usize::from(area.width);
    let offset = match line.alignment {
        Some(Alignment::Center) => available.saturating_sub(line_width) / 2,
        Some(Alignment::Right) => available.saturating_sub(line_width),
        Some(Alignment::Left) | None => 0,
    };
    let mut x = area
        .left()
        .saturating_add(u16::try_from(offset).unwrap_or(u16::MAX));
    for cell in cells {
        if x >= area.right()
            || x.saturating_add(u16::try_from(cell.width).unwrap_or(u16::MAX)) > area.right()
        {
            break;
        }
        let symbol = cell
            .target
            .as_deref()
            .map_or_else(|| cell.text.clone(), |target| hyperlink(&cell.text, target));
        buf[Position::new(x, area.top())]
            .set_symbol(&symbol)
            .set_style(cell.style);
        for hidden in 1..cell.width {
            if let Some(hidden_cell) = buf.cell_mut(Position::new(
                x.saturating_add(u16::try_from(hidden).unwrap_or(u16::MAX)),
                area.top(),
            )) {
                hidden_cell.reset();
            }
        }
        x = x.saturating_add(u16::try_from(cell.width).unwrap_or(u16::MAX));
    }
}

#[derive(Debug)]
struct BufferedHyperlinkCell {
    text: String,
    target: Option<String>,
    style: Style,
    width: usize,
}

/// Parse one OSC 8 control sequence at a byte boundary. Returns the byte
/// after the sequence and `Some(target)` for an opener, or `None` for the
/// empty-target closer. Both BEL and ST terminators are accepted because the
/// terminal protocol permits either form.
pub(crate) fn osc8_control_at(text: &str, start: usize) -> Option<(usize, Option<String>)> {
    const PREFIX: &str = "\x1b]8;;";
    if !text[start..].starts_with(PREFIX) {
        return None;
    }
    let payload_start = start + PREFIX.len();
    let bytes = text.as_bytes();
    let mut cursor = payload_start;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\x07' => {
                let target = (!text[payload_start..cursor].is_empty())
                    .then(|| text[payload_start..cursor].to_string());
                return Some((cursor + 1, target));
            }
            b'\x1b' if bytes.get(cursor + 1) == Some(&b'\\') => {
                let target = (!text[payload_start..cursor].is_empty())
                    .then(|| text[payload_start..cursor].to_string());
                return Some((cursor + 2, target));
            }
            _ => cursor += 1,
        }
    }
    None
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

    #[test]
    fn hyperlinked_line_keeps_metadata_out_of_visible_cells() {
        let target = "https://example.com/docs";
        let line = Line::from(vec![
            Span::raw("see "),
            Span::raw(hyperlink("docs", target)),
            Span::raw(" now"),
        ]);
        let area = Rect::new(0, 0, 12, 1);
        let mut buffer = Buffer::empty(area);

        render_line_with_hyperlinks(&line, area, &mut buffer);

        assert_eq!(buffer.cell(Position::new(0, 0)).unwrap().symbol(), "s");
        assert_eq!(
            buffer.cell(Position::new(4, 0)).unwrap().symbol(),
            &hyperlink("d", target)
        );
        assert_eq!(
            buffer.cell(Position::new(5, 0)).unwrap().symbol(),
            &hyperlink("o", target)
        );
        assert_eq!(
            buffer.cell(Position::new(6, 0)).unwrap().symbol(),
            &hyperlink("c", target)
        );
        assert_eq!(
            buffer.cell(Position::new(7, 0)).unwrap().symbol(),
            &hyperlink("s", target)
        );
        assert_eq!(buffer.cell(Position::new(8, 0)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell(Position::new(9, 0)).unwrap().symbol(), "n");
        assert_eq!(buffer.cell(Position::new(10, 0)).unwrap().symbol(), "o");
        assert_eq!(buffer.cell(Position::new(11, 0)).unwrap().symbol(), "w");
    }
}
