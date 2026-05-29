//! `VirtualMessageList` — windowed scrollback (M7-03).
//!
//! Replaces M6's capped-500 [`Scrollback`](crate::components::scrollback).
//! The full message log is retained; only the lines intersecting the
//! viewport (+ overscan) are rendered each frame.
//!
//! ## Scroll model (LINES, not message rows)
//! - `scroll_offset` = lines scrolled up from the bottom.
//! - `offset = 0` → the last `viewport_height` lines (latest) are shown.
//! - `max_offset = total_lines.saturating_sub(viewport_height)` shows the
//!   oldest content.
//!
//! ## Height cache
//! A [`HeightCache`] maps message-index → rendered line count at a given
//! viewport width. Variable-height messages (a 1-line text vs a 200-line
//! diff) make a line-based model mandatory. The cache is invalidated and
//! recomputed whenever the viewport width changes.

use std::collections::HashMap;

use iocraft::prelude::*;
use lingxi_protocol::ToolUseId;
use unicode_width::UnicodeWidthStr;

use crate::state::RenderedMessage;

// Re-export the per-variant message dispatch from `scrollback` so the
// windowed renderer reuses the exact same per-variant rendering
// (including M7-02's StructuredDiff branch) without duplicating it.
pub use crate::components::scrollback::render_message;

/// Overscan: render this many extra lines above and below the viewport so
/// a fast line-step doesn't flash blank rows.
pub const OVERSCAN_LINES: usize = 3;

/// Measure the rendered height (line count) of one message at a given
/// viewport width. Counts explicit `\n`-separated rows and adds wrap rows
/// for any line wider than `width` (unicode display columns). A message
/// always occupies at least one line.
#[must_use]
pub fn measured_height(msg: &RenderedMessage, width: usize) -> usize {
    let text = render_text_for_measure(msg);
    if text.is_empty() {
        return 1;
    }
    let w = width.max(1);
    let mut rows = 0usize;
    for line in text.split('\n') {
        let cols = UnicodeWidthStr::width(line);
        // A blank logical line still occupies one row.
        rows += (cols / w) + usize::from(cols % w != 0 || cols == 0);
    }
    rows.max(1)
}

/// Project a message to the plain text used for height measurement. This
/// mirrors what each renderer prints to screen at the line level (prefixes
/// add columns but not rows for these single-line-prefixed variants).
fn render_text_for_measure(msg: &RenderedMessage) -> String {
    match msg {
        RenderedMessage::UserText { body, .. }
        | RenderedMessage::AssistantText { body, .. }
        | RenderedMessage::SystemText { body, .. } => body.clone(),
        RenderedMessage::AssistantToolUse { tool, .. } => format!("● {tool}(…)"),
        RenderedMessage::UserToolResult { result, .. } => result
            .as_str()
            .map_or_else(|| result.to_string(), str::to_string),
    }
}

/// Per-message rendered-height cache. Maps message-index → line count at a
/// fixed viewport width. Backs all scroll-offset math. Rebuilt on width
/// change; appended to as new messages arrive (callers may simply rebuild
/// — `build` is O(n) over a 5k log and runs at most once per width change).
#[derive(Debug, Clone, Default)]
pub struct HeightCache {
    heights: Vec<usize>,
    total: usize,
    width: usize,
}

impl HeightCache {
    /// Build the cache for `messages` at `width` columns.
    #[must_use]
    pub fn build(messages: &[RenderedMessage], width: usize) -> Self {
        let heights: Vec<usize> = messages.iter().map(|m| measured_height(m, width)).collect();
        let total = heights.iter().sum();
        Self {
            heights,
            total,
            width,
        }
    }

    /// Rebuild in place at a new width (or after the log changed).
    pub fn recompute(&mut self, messages: &[RenderedMessage], width: usize) {
        *self = Self::build(messages, width);
    }

    /// Line count for the message at `index`, or 0 if out of range.
    #[must_use]
    pub fn height_at(&self, index: usize) -> usize {
        self.heights.get(index).copied().unwrap_or(0)
    }

    /// Sum of all message heights (total rendered lines).
    #[must_use]
    pub fn total_lines(&self) -> usize {
        self.total
    }

    /// Number of cached messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.heights.len()
    }

    /// True when no messages are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// Width the cache was last built for.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }
}

/// The result of windowing: which messages intersect the viewport and how
/// many lines of the first/last message to skip/take. `skip_top_lines` are
/// the lines of `first_index`'s message hidden above the viewport top;
/// `take_lines` is the total number of rendered lines the viewport holds
/// (after `skip_top_lines`), spanning `first_index..=last_index`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowSlice {
    /// First message index intersecting the viewport (inclusive).
    pub first_index: usize,
    /// Last message index intersecting the viewport (inclusive).
    pub last_index: usize,
    /// Lines of `first_index`'s message hidden above the viewport top.
    pub skip_top_lines: usize,
    /// Total visible line budget across the window.
    pub take_lines: usize,
    /// True when nothing is visible (empty log / zero viewport).
    pub empty: bool,
}

impl WindowSlice {
    /// Inclusive range of message indices in the window. Empty iterator
    /// when [`Self::is_empty`].
    pub fn indices(&self) -> impl Iterator<Item = usize> {
        let (lo, hi) = if self.empty {
            (1usize, 0usize) // empty range
        } else {
            (self.first_index, self.last_index)
        };
        lo..=hi
    }

    /// True when the window holds no messages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.empty
    }
}

/// Core windowing function. Given the full `messages`, their `cache`d
/// heights, a line-based `scroll_offset`, and the `viewport_height` in
/// lines, return the contiguous message slice intersecting the viewport
/// plus the first-message top-skip and the total visible line budget.
///
/// `scroll_offset` is clamped here defensively, but callers
/// ([`crate::app::scroll_with_viewport`]) clamp it on input.
#[must_use]
pub fn render_window(
    messages: &[RenderedMessage],
    cache: &HeightCache,
    scroll_offset: usize,
    viewport_height: usize,
) -> WindowSlice {
    if messages.is_empty() || viewport_height == 0 || cache.total_lines() == 0 {
        return WindowSlice {
            empty: true,
            ..WindowSlice::default()
        };
    }
    let total = cache.total_lines();
    let max_offset = total.saturating_sub(viewport_height);
    let offset = scroll_offset.min(max_offset);

    // The visible line range is [top_line, bottom_line) in absolute lines
    // from the top of the log.
    let bottom_line = total - offset;
    let top_line = bottom_line.saturating_sub(viewport_height);

    // Walk messages accumulating line spans; find the first/last whose
    // span intersects [top_line, bottom_line).
    let mut acc = 0usize; // absolute line at the start of the current msg
    let mut first_index = 0usize;
    let mut skip_top_lines = 0usize;
    let mut last_index = 0usize;
    let mut found_first = false;
    for (i, _m) in messages.iter().enumerate() {
        let h = cache.height_at(i);
        let span_start = acc;
        let span_end = acc + h; // exclusive
                                // Intersects the viewport if span_end > top_line && span_start < bottom_line.
        if span_end > top_line && span_start < bottom_line {
            if !found_first {
                first_index = i;
                skip_top_lines = top_line.saturating_sub(span_start);
                found_first = true;
            }
            last_index = i;
        }
        acc = span_end;
        if span_start >= bottom_line {
            break;
        }
    }

    WindowSlice {
        first_index,
        last_index,
        skip_top_lines,
        take_lines: viewport_height.min(total),
        empty: false,
    }
}

/// Props for [`VirtualMessageList`]. Mirrors the M6 `ScrollbackProps`
/// surface plus the line-based viewport. The full `messages` log is
/// passed; the component windows it.
#[derive(Default, Props)]
pub struct VirtualMessageListProps {
    /// Full retained message log (clone of `AppState::messages`).
    pub messages: Vec<RenderedMessage>,
    /// Line-based scroll offset (0 = latest at bottom).
    pub scroll_offset: usize,
    /// Viewport height in lines.
    pub viewport_height: usize,
    /// Viewport width in columns (drives the height cache).
    pub viewport_width: usize,
    /// Per-tool expanded flags (clone of `AppState::expanded`).
    pub expanded: HashMap<ToolUseId, bool>,
    /// Focused tool id (clone of `AppState::focused_tool_id`).
    pub focused_tool_id: Option<ToolUseId>,
}

/// Windowed scrollback component. Renders only the messages whose line
/// spans intersect the viewport (+ overscan), not the whole log.
#[component]
pub fn VirtualMessageList(props: &VirtualMessageListProps) -> impl Into<AnyElement<'static>> {
    let width = props.viewport_width.max(1);
    let cache = HeightCache::build(&props.messages, width);
    // Overscan: render a few extra lines of viewport so a line-step does
    // not flash blank rows. Purely a visual buffer — offset math is exact.
    let vh = props.viewport_height.saturating_add(OVERSCAN_LINES);
    let win = render_window(&props.messages, &cache, props.scroll_offset, vh);

    let expanded = props.expanded.clone();
    let focused_tool_id = props.focused_tool_id;
    let rendered: Vec<AnyElement<'static>> = if win.is_empty() {
        Vec::new()
    } else {
        win.indices()
            .filter_map(|i| props.messages.get(i).cloned())
            .map(|m| render_message(m, &expanded, focused_tool_id))
            .collect()
    };
    element! {
        View(flex_direction: FlexDirection::Column, flex_grow: 1.0) {
            #(rendered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RenderedMessage;

    fn user(body: &str) -> RenderedMessage {
        RenderedMessage::UserText {
            body: body.to_string(),
            timestamp: 0,
        }
    }

    #[test]
    fn single_line_message_measures_one() {
        assert_eq!(measured_height(&user("hi"), 80), 1);
    }

    #[test]
    fn three_newlines_measure_three_lines() {
        assert_eq!(measured_height(&user("a\nb\nc"), 80), 3);
    }

    #[test]
    fn long_line_wraps_at_width() {
        // 25 chars at width 10 → ceil(25/10) = 3 rows.
        let body = "x".repeat(25);
        assert_eq!(measured_height(&user(&body), 10), 3);
    }

    #[test]
    fn empty_body_measures_one() {
        assert_eq!(measured_height(&user(""), 80), 1);
    }

    #[test]
    fn height_cache_builds_per_index_and_total() {
        let msgs = vec![user("one"), user("a\nb\nc"), user("two")];
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.height_at(0), 1);
        assert_eq!(cache.height_at(1), 3);
        assert_eq!(cache.height_at(2), 1);
        assert_eq!(cache.total_lines(), 5);
        assert_eq!(cache.width(), 80);
    }

    #[test]
    fn height_cache_recomputes_on_width_change() {
        let msgs = vec![user(&"x".repeat(20))]; // 20 cols
        let mut cache = HeightCache::build(&msgs, 80); // ceil(20/80) = 1
        assert_eq!(cache.total_lines(), 1);
        cache.recompute(&msgs, 10); // ceil(20/10) = 2
        assert_eq!(cache.total_lines(), 2);
        assert_eq!(cache.width(), 10);
    }

    #[test]
    fn height_cache_empty_is_zero_total() {
        let cache = HeightCache::build(&[], 80);
        assert_eq!(cache.total_lines(), 0);
    }

    #[test]
    fn window_at_offset_zero_shows_tail_lines() {
        // 4 messages of height [1,1,1,1] = 4 total lines; viewport 3.
        let msgs = vec![user("m0"), user("m1"), user("m2"), user("m3")];
        let cache = HeightCache::build(&msgs, 80);
        let win = render_window(&msgs, &cache, 0, 3);
        // bottom_line = 4, top_line = 1 → messages 1..=3 visible (m1,m2,m3).
        assert_eq!(win.first_index, 1);
        assert_eq!(win.last_index, 3);
        assert_eq!(win.skip_top_lines, 0);
        assert_eq!(win.take_lines, 3);
        assert_eq!(win.indices().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn window_empty_log_is_empty() {
        let cache = HeightCache::build(&[], 80);
        let win = render_window(&[], &cache, 0, 5);
        assert!(win.is_empty());
    }

    #[test]
    fn window_zero_viewport_is_empty() {
        let msgs = vec![user("m0")];
        let cache = HeightCache::build(&msgs, 80);
        let win = render_window(&msgs, &cache, 0, 0);
        assert!(win.is_empty());
    }

    // Heights: m0=1, m1=50, m2=1, m3=1  → total = 53 lines.
    fn mixed_log() -> Vec<RenderedMessage> {
        vec![
            user("m0"),                      // 1 line
            user(&vec!["x"; 50].join("\n")), // 50 lines
            user("m2"),                      // 1 line
            user("m3"),                      // 1 line
        ]
    }

    #[test]
    fn mixed_offset_zero_shows_tail_into_tall_message() {
        let msgs = mixed_log();
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.total_lines(), 53);
        // viewport 10, offset 0 → top_line = 43, bottom_line = 53.
        // m1 spans [1,51), m2 [51,52), m3 [52,53).
        let win = render_window(&msgs, &cache, 0, 10);
        assert_eq!(win.first_index, 1); // tall message is partly visible
        assert_eq!(win.last_index, 3);
        assert_eq!(win.skip_top_lines, 42); // hide first 42 of m1's 50 lines
    }

    #[test]
    fn mixed_scrolled_into_tall_message_middle() {
        let msgs = mixed_log();
        let cache = HeightCache::build(&msgs, 80);
        // offset 20 → bottom_line = 33, top_line = 23. Only m1 (spans [1,51)).
        let win = render_window(&msgs, &cache, 20, 10);
        assert_eq!(win.first_index, 1);
        assert_eq!(win.last_index, 1);
        assert_eq!(win.skip_top_lines, 22); // top_line(23) - span_start(1) = 22
    }

    #[test]
    fn mixed_scrolled_to_top_shows_first_message() {
        let msgs = mixed_log();
        let cache = HeightCache::build(&msgs, 80);
        // max_offset = 53 - 10 = 43. offset 43 → top_line 0, bottom_line 10.
        let win = render_window(&msgs, &cache, 43, 10);
        assert_eq!(win.first_index, 0);
        assert_eq!(win.skip_top_lines, 0);
        // m0 [0,1), m1 [1,51) → window covers m0 and start of m1.
        assert_eq!(win.last_index, 1);
    }

    #[test]
    fn mixed_offset_over_max_is_clamped() {
        let msgs = mixed_log();
        let cache = HeightCache::build(&msgs, 80);
        // offset 9999 clamps to max_offset 43 → identical to the top window.
        let win = render_window(&msgs, &cache, 9999, 10);
        assert_eq!(win.first_index, 0);
        assert_eq!(win.skip_top_lines, 0);
    }

    #[test]
    fn window_render_count_bounded_by_viewport_not_log_size() {
        // 5000 single-line messages, viewport 20 → window holds ~20 (+overscan),
        // never 5000.
        let msgs: Vec<RenderedMessage> = (0..5000).map(|i| user(&format!("m{i}"))).collect();
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.total_lines(), 5000);
        let win = render_window(&msgs, &cache, 0, 20);
        let count = win.indices().count();
        assert!(
            count <= 21,
            "window rendered {count} messages, expected <= 21"
        );
        assert!(count >= 20, "window should fill the viewport, got {count}");
    }
}
