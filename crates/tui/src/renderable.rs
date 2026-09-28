//! The shared renderable contract for `tui` widgets (plan Phase 2).
//!
//! Ported from codex-rs `tui/src/render/renderable.rs` (UI architecture
//! pattern only — no codex product types). A [`Renderable`] draws itself into
//! `(Rect, &mut Buffer)` instead of owning a full frame: only the terminal
//! draw boundary ([`crate::chat_widget::ChatWidget::render_frame`]) sees a
//! [`crate::terminal::Frame`], and it adapts by handing the frame's buffer to
//! renderables and copying the winning cursor position/style back onto the
//! frame afterwards.
//!
//! [`ColumnRenderable`] is the vertical stacking helper: children are laid out
//! top-to-bottom at their [`Renderable::desired_height`], and the cursor
//! position/style delegate to the first child that claims a cursor.

use crossterm::cursor::SetCursorStyle;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;

/// Something that can draw itself into a rect of a buffer, report the height
/// it wants at a given width, and (optionally) claim a terminal cursor.
pub trait Renderable {
    /// Draw into `area` of `buf`.
    fn render(&self, area: Rect, buf: &mut Buffer);
    /// The number of rows this renderable wants when given `width` columns.
    fn desired_height(&self, width: u16) -> u16;
    /// Where the terminal cursor should be shown while this renderable has
    /// focus, in absolute buffer coordinates (`None` → cursor hidden).
    fn cursor_pos(&self, _area: Rect) -> Option<(u16, u16)> {
        None
    }
    /// The cursor style applied when [`Self::cursor_pos`] returns `Some`.
    fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
        SetCursorStyle::DefaultUserShape
    }
}

/// An owned-or-borrowed child of a composite renderable (codex
/// `RenderableItem`): lets containers mix boxed children with borrows without
/// forcing either allocation or lifetime gymnastics on the caller.
pub enum RenderableItem<'a> {
    /// A boxed child owned by the container.
    Owned(Box<dyn Renderable + 'a>),
    /// A child borrowed from elsewhere.
    Borrowed(&'a dyn Renderable),
}

impl RenderableItem<'_> {
    fn as_renderable(&self) -> &dyn Renderable {
        match self {
            RenderableItem::Owned(child) => child.as_ref(),
            RenderableItem::Borrowed(child) => *child,
        }
    }
}

impl Renderable for RenderableItem<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.as_renderable().render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.as_renderable().desired_height(width)
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.as_renderable().cursor_pos(area)
    }

    fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        self.as_renderable().cursor_style(area)
    }
}

impl<'a> From<Box<dyn Renderable + 'a>> for RenderableItem<'a> {
    fn from(value: Box<dyn Renderable + 'a>) -> Self {
        RenderableItem::Owned(value)
    }
}

impl<'a, R> From<R> for Box<dyn Renderable + 'a>
where
    R: Renderable + 'a,
{
    fn from(value: R) -> Self {
        Box::new(value)
    }
}

/// Renders nothing and wants no rows (useful as a placeholder child).
impl Renderable for () {
    fn render(&self, _area: Rect, _buf: &mut Buffer) {}
    fn desired_height(&self, _width: u16) -> u16 {
        0
    }
}

/// A single unwrapped row of plain text.
impl Renderable for &str {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Widget::render(*self, area, buf);
    }
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// A single unwrapped row of plain text.
impl Renderable for String {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Widget::render(self.as_str(), area, buf);
    }
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// A single unwrapped styled span.
impl Renderable for Span<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_ref(area, buf);
    }
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// A single unwrapped styled line.
impl Renderable for Line<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        WidgetRef::render_ref(self, area, buf);
    }
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// A paragraph; its desired height is its wrapped line count at the given
/// width (`Paragraph::line_count`, the codex height contract).
impl Renderable for Paragraph<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_ref(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        u16::try_from(self.line_count(width)).unwrap_or(u16::MAX)
    }
}

/// `None` renders nothing at zero height; `Some` delegates.
impl<R: Renderable> Renderable for Option<R> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if let Some(renderable) = self {
            renderable.render(area, buf);
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.as_ref()
            .map_or(0, |renderable| renderable.desired_height(width))
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.as_ref()
            .and_then(|renderable| renderable.cursor_pos(area))
    }

    fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        self.as_ref()
            .map_or(SetCursorStyle::DefaultUserShape, |renderable| {
                renderable.cursor_style(area)
            })
    }
}

/// Vertical stacking: children are laid out top-to-bottom, each given its
/// [`Renderable::desired_height`] (clipped to the available area), and the
/// cursor delegates to the first child that claims one.
#[derive(Default)]
pub struct ColumnRenderable<'a> {
    children: Vec<RenderableItem<'a>>,
}

impl<'a> ColumnRenderable<'a> {
    /// An empty column.
    #[must_use]
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }

    /// A column over owned `children` (any [`Renderable`], auto-boxed).
    #[must_use]
    pub fn with<I, T>(children: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Box<dyn Renderable + 'a>>,
    {
        Self {
            children: children
                .into_iter()
                .map(|child| RenderableItem::Owned(child.into()))
                .collect(),
        }
    }

    /// Append an owned child.
    pub fn push(&mut self, child: impl Into<Box<dyn Renderable + 'a>>) {
        self.children.push(RenderableItem::Owned(child.into()));
    }

    /// The rect each child occupies within `area` (its desired height at the
    /// area's width, clipped to the area).
    fn child_areas(&self, area: Rect) -> Vec<Rect> {
        let mut areas = Vec::with_capacity(self.children.len());
        let mut y = area.y;
        for child in &self.children {
            let child_area = Rect::new(area.x, y, area.width, child.desired_height(area.width))
                .intersection(area);
            y += child_area.height;
            areas.push(child_area);
        }
        areas
    }
}

impl Renderable for ColumnRenderable<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        for (child, child_area) in self.children.iter().zip(self.child_areas(area)) {
            if !child_area.is_empty() {
                child.render(child_area, buf);
            }
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.children
            .iter()
            .map(|child| child.desired_height(width))
            .sum()
    }

    /// The cursor position of the first child that claims one, at the child's
    /// position in the column. It is generally assumed that either zero or
    /// one child claims a cursor.
    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        for (child, child_area) in self.children.iter().zip(self.child_areas(area)) {
            if child_area.is_empty() {
                continue;
            }
            if let Some(pos) = child.cursor_pos(child_area) {
                return Some(pos);
            }
        }
        None
    }

    fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        for (child, child_area) in self.children.iter().zip(self.child_areas(area)) {
            if !child_area.is_empty() && child.cursor_pos(child_area).is_some() {
                return child.cursor_style(child_area);
            }
        }
        SetCursorStyle::DefaultUserShape
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Position;
    use ratatui::widgets::Wrap;

    use super::*;

    /// A fixed-height renderable that fills its rows with `ch` and optionally
    /// claims a cursor at a relative offset with a distinctive style.
    struct FixedBlock {
        height: u16,
        ch: char,
        cursor: Option<(u16, u16)>,
    }

    impl FixedBlock {
        fn new(height: u16, ch: char) -> Self {
            Self {
                height,
                ch,
                cursor: None,
            }
        }

        fn with_cursor(height: u16, ch: char, dx: u16, dy: u16) -> Self {
            Self {
                height,
                ch,
                cursor: Some((dx, dy)),
            }
        }
    }

    impl Renderable for FixedBlock {
        fn render(&self, area: Rect, buf: &mut Buffer) {
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    buf.cell_mut(Position::new(x, y))
                        .expect("cell in area")
                        .set_char(self.ch);
                }
            }
        }

        fn desired_height(&self, _width: u16) -> u16 {
            self.height
        }

        fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
            self.cursor.map(|(dx, dy)| (area.x + dx, area.y + dy))
        }

        fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
            SetCursorStyle::SteadyBar
        }
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (buf.area.left()..buf.area.right())
            .map(|x| {
                buf.cell(Position::new(x, y))
                    .map_or(" ", ratatui::buffer::Cell::symbol)
            })
            .collect()
    }

    #[test]
    fn column_stacks_children_vertically_and_sums_desired_height() {
        let column = ColumnRenderable::with([FixedBlock::new(1, 'a'), FixedBlock::new(2, 'b')]);
        assert_eq!(column.desired_height(10), 3);

        let area = Rect::new(0, 0, 4, 4);
        let mut buf = Buffer::empty(area);
        column.render(area, &mut buf);
        assert_eq!(row(&buf, 0), "aaaa");
        assert_eq!(row(&buf, 1), "bbbb");
        assert_eq!(row(&buf, 2), "bbbb");
        assert_eq!(row(&buf, 3), "    ", "rows past the stack stay untouched");
    }

    #[test]
    fn column_stacks_at_an_offset_area_origin() {
        let column = ColumnRenderable::with([FixedBlock::new(1, 'a'), FixedBlock::new(1, 'b')]);
        let area = Rect::new(2, 5, 3, 2);
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 8));
        column.render(area, &mut buf);
        assert_eq!(row(&buf, 5), "  aaa   ");
        assert_eq!(row(&buf, 6), "  bbb   ");
    }

    #[test]
    fn column_clips_children_to_the_area_without_panicking() {
        let column = ColumnRenderable::with([FixedBlock::new(2, 'a'), FixedBlock::new(2, 'b')]);
        // Area holds 3 of the 4 desired rows: the second child is clipped.
        let area = Rect::new(0, 0, 2, 3);
        let mut buf = Buffer::empty(area);
        column.render(area, &mut buf);
        assert_eq!(row(&buf, 0), "aa");
        assert_eq!(row(&buf, 1), "aa");
        assert_eq!(row(&buf, 2), "bb");
    }

    #[test]
    fn column_cursor_delegates_to_first_child_that_claims_one() {
        let column = ColumnRenderable::with([
            FixedBlock::new(2, 'a'),
            FixedBlock::with_cursor(1, 'b', 3, 0),
        ]);
        let area = Rect::new(1, 4, 10, 5);
        // The cursor child sits below the 2-row first child: y = 4 + 2.
        assert_eq!(column.cursor_pos(area), Some((4, 6)));
        assert!(matches!(
            column.cursor_style(area),
            SetCursorStyle::SteadyBar
        ));
    }

    #[test]
    fn column_without_cursor_children_reports_none_and_default_style() {
        let column = ColumnRenderable::with([FixedBlock::new(1, 'a')]);
        let area = Rect::new(0, 0, 4, 4);
        assert_eq!(column.cursor_pos(area), None);
        assert!(matches!(
            column.cursor_style(area),
            SetCursorStyle::DefaultUserShape
        ));
    }

    #[test]
    fn column_push_and_borrowed_items_render_alike() {
        let borrowed = FixedBlock::new(1, 'x');
        let mut column = ColumnRenderable::new();
        column.push(FixedBlock::new(1, 'o'));
        column.children.push(RenderableItem::Borrowed(&borrowed));
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        column.render(area, &mut buf);
        assert_eq!(row(&buf, 0), "oo");
        assert_eq!(row(&buf, 1), "xx");
        assert_eq!(column.desired_height(2), 2);
    }

    #[test]
    fn option_none_is_zero_height_and_renders_nothing() {
        let none: Option<FixedBlock> = None;
        assert_eq!(none.desired_height(10), 0);
        let area = Rect::new(0, 0, 2, 1);
        let mut buf = Buffer::empty(area);
        none.render(area, &mut buf);
        assert_eq!(row(&buf, 0), "  ");
        assert_eq!(none.cursor_pos(area), None);

        let some = Some(FixedBlock::with_cursor(1, 'z', 0, 0));
        assert_eq!(some.desired_height(10), 1);
        assert_eq!(some.cursor_pos(area), Some((0, 0)));
    }

    #[test]
    fn line_and_string_are_single_row_renderables() {
        let line = Line::from("hi");
        assert_eq!(line.desired_height(80), 1);
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        line.render(area, &mut buf);
        assert_eq!(row(&buf, 0), "hi  ");
        assert_eq!("s".desired_height(80), 1);
        assert_eq!(String::from("s").desired_height(80), 1);
    }

    #[test]
    fn paragraph_desired_height_counts_wrapped_lines() {
        let paragraph = Paragraph::new("one two three").wrap(Wrap { trim: false });
        assert_eq!(paragraph.desired_height(5), 3, "wraps to one word per row");
        assert_eq!(paragraph.desired_height(80), 1);
    }
}
