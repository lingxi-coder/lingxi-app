//! Full-screen terminal selection.
//!
//! Inline mode deliberately leaves mouse capture disabled so iTerm2 and other
//! terminals retain their native selection.  Full-screen mode owns mouse
//! events, so it keeps a small buffer-coordinate selection and extracts text
//! with display-cell awareness (wide CJK/emoji continuation cells are never
//! duplicated).

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

/// Mouse selection state for the current full-screen frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionState {
    anchor: Option<Position>,
    cursor: Option<Position>,
    dragging: bool,
}

impl SelectionState {
    /// Start a selection, clamping the terminal position into `area`.
    pub fn begin(&mut self, position: Position, area: Rect) {
        let position = clamp_position(position, area);
        self.anchor = Some(position);
        self.cursor = Some(position);
        self.dragging = true;
    }

    /// Extend the current selection while the primary button is held.
    pub fn update(&mut self, position: Position, area: Rect) {
        if self.dragging {
            self.cursor = Some(clamp_position(position, area));
        }
    }

    /// Finish the drag and return the selected text.
    ///
    /// Empty/whitespace-only selections return `None`, matching native
    /// copy-on-select behavior (a click without a drag must not erase the
    /// clipboard).
    pub fn finish(&mut self, buffer: &Buffer) -> Option<String> {
        self.dragging = false;
        let (anchor, cursor) = (self.anchor.take()?, self.cursor.take()?);
        let text = extract_selection(buffer, anchor, cursor);
        (!text.trim().is_empty()).then_some(text)
    }

    /// Finish a drag at the button-release position.
    ///
    /// Some terminals emit the final coordinate only on `MouseEventKind::Up`
    /// without a preceding drag event. Including that coordinate prevents the
    /// copied range from stopping one or more cells before the visible release
    /// point.
    pub fn finish_at(&mut self, position: Position, area: Rect, buffer: &Buffer) -> Option<String> {
        self.update(position, area);
        self.finish(buffer)
    }

    /// Cancel an unfinished drag (resize, mode switch, or terminal reset).
    pub fn clear(&mut self) {
        self.anchor = None;
        self.cursor = None;
        self.dragging = false;
    }

    /// Whether a primary-button drag is active.
    #[must_use]
    pub const fn is_dragging(&self) -> bool {
        self.dragging
    }
}

fn clamp_position(position: Position, area: Rect) -> Position {
    if area.is_empty() {
        return area.as_position();
    }
    Position::new(
        position
            .x
            .clamp(area.left(), area.right().saturating_sub(1)),
        position
            .y
            .clamp(area.top(), area.bottom().saturating_sub(1)),
    )
}

/// Extract a row-major selection from the rendered terminal buffer.
#[must_use]
pub fn extract_selection(buffer: &Buffer, start: Position, end: Position) -> String {
    let area = buffer.area;
    if area.is_empty() {
        return String::new();
    }
    let mut start = clamp_position(start, area);
    let mut end = clamp_position(end, area);
    if (start.y, start.x) > (end.y, end.x) {
        std::mem::swap(&mut start, &mut end);
    }
    start.x = snap_to_wide_lead(buffer, start);
    end.x = snap_to_wide_lead(buffer, end);

    let mut lines = Vec::new();
    for y in start.y..=end.y {
        let row_start = if y == start.y { start.x } else { area.left() };
        let row_end = if y == end.y {
            end.x
        } else {
            area.right().saturating_sub(1)
        };
        let mut line = String::new();
        let mut covered_until = row_start;
        for x in row_start..=row_end {
            // A glyph whose lead cell was already emitted covers this display
            // column.  Ratatui continuation cells are otherwise blank and
            // would become spurious spaces after CJK/emoji.
            if x < covered_until {
                continue;
            }
            let symbol = buffer[(x, y)].symbol();
            if symbol.is_empty() {
                continue;
            }
            line.push_str(symbol);
            let width = u16::try_from(symbol.width().max(1)).unwrap_or(u16::MAX);
            covered_until = x.saturating_add(width);
        }
        lines.push(line.trim_end_matches(' ').to_string());
    }
    lines.join("\n")
}

fn snap_to_wide_lead(buffer: &Buffer, position: Position) -> u16 {
    let area = buffer.area;
    let mut x = position.x;
    while x > area.left() {
        let previous_x = x - 1;
        let previous = buffer[(previous_x, position.y)].symbol();
        let width = u16::try_from(previous.width()).unwrap_or(u16::MAX);
        if width > 1 && previous_x.saturating_add(width) > position.x {
            return previous_x;
        }
        if !previous.is_empty() && previous != " " {
            break;
        }
        x = previous_x;
    }
    position.x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer() -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 3));
        buffer.set_string(0, 0, "ab武汉cd", ratatui::style::Style::default());
        buffer.set_string(0, 1, "second", ratatui::style::Style::default());
        buffer
    }

    #[test]
    fn row_selection_does_not_duplicate_wide_continuation_cells() {
        let buffer = buffer();
        assert_eq!(
            extract_selection(&buffer, Position::new(0, 0), Position::new(9, 0)),
            "ab武汉cd"
        );
    }

    #[test]
    fn selecting_a_wide_continuation_snaps_to_the_lead_cell() {
        let buffer = buffer();
        assert_eq!(
            extract_selection(&buffer, Position::new(3, 0), Position::new(5, 0)),
            "武汉"
        );
    }

    #[test]
    fn reverse_multiline_selection_is_normalized() {
        let buffer = buffer();
        assert_eq!(
            extract_selection(&buffer, Position::new(2, 1), Position::new(7, 0)),
            "d\nsec"
        );
    }

    #[test]
    fn click_without_text_does_not_copy() {
        let buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        let mut selection = SelectionState::default();
        selection.begin(Position::new(1, 0), buffer.area);
        assert_eq!(selection.finish(&buffer), None);
    }

    #[test]
    fn button_release_coordinate_completes_the_selected_range() {
        let buffer = buffer();
        let mut selection = SelectionState::default();
        selection.begin(Position::new(0, 0), buffer.area);
        assert_eq!(
            selection.finish_at(Position::new(9, 0), buffer.area, &buffer),
            Some("ab武汉cd".to_string())
        );
    }
}
