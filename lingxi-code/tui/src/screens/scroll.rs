//! Shared scroll helper for the interactive-screens batch.
//!
//! `resume.rs`/`agents.rs` each re-implement Up/Down + clamp by hand. This
//! module factors the *list-window* scrolling shared by full-page modal
//! viewers into one small, terminal-free piece: a [`ScrollState`] (top-of-
//! window offset + content length + viewport height), a pure
//! [`ScrollState::handle_scroll_key`] reducer, and a [`visible_window`]
//! render helper that returns the on-screen slice plus an optional scroll
//! indicator line.
//!
//! Scope is SCROLLING ONLY — no selection cursor, no generic
//! select/confirm/text dialog. A caller that also tracks a highlighted row
//! (resume/agents) keeps its own `selected` index and may drive the offset
//! from it; this helper does not own selection.
//!
//! Key semantics are byte-locked to the claude-code modal pager bindings
//! (`src/keybindings/defaultBindings.ts`):
//! `pageup → scroll:pageUp`, `pagedown → scroll:pageDown`,
//! `ctrl+home → scroll:top`, `ctrl+end → scroll:bottom`. Bare `Up`/`Down`
//! line-step; bare `Home`/`End` jump to top/bottom (the natural list-viewer
//! convention, and what a modal without a text cursor expects). The offset
//! is the index of the FIRST visible row and grows downward, so `Down`
//! increases it — the same direction resume/agents move selection.
//!
//! Every transition uses SATURATING clamps: the offset is held in
//! `0..=len.saturating_sub(viewport)`, so it can never index past the last
//! full window, and an empty or short list (len ≤ viewport) pins the offset
//! at 0.
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent};

/// Scroll position for a fixed-height window over a flat list.
///
/// Invariant (re-established after every setter / key): `offset` is clamped
/// into `0..=max_offset()`, where `max_offset() == len.saturating_sub(viewport)`.
/// A `viewport` of 0 is tolerated (degenerate — shows nothing, `max_offset`
/// is `len`); callers normally pass a viewport ≥ 1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollState {
    /// Index of the first visible row (top of the window). Always
    /// `≤ max_offset()`.
    offset: usize,
    /// Total number of rows in the backing list.
    len: usize,
    /// Number of rows the window can show at once.
    viewport: usize,
}

impl ScrollState {
    /// Build a state for `len` rows shown `viewport` at a time, anchored at
    /// the top (offset 0).
    #[must_use]
    pub fn new(len: usize, viewport: usize) -> Self {
        Self {
            offset: 0,
            len,
            viewport,
        }
    }

    /// Current top-of-window offset (index of the first visible row).
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Total row count of the backing list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` when the backing list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Window height (rows shown at once).
    #[must_use]
    pub fn viewport(&self) -> usize {
        self.viewport
    }

    /// Largest legal offset: `len - viewport`, saturating at 0 when the list
    /// fits entirely in the window (or is empty).
    #[must_use]
    pub fn max_offset(&self) -> usize {
        self.len.saturating_sub(self.viewport)
    }

    /// `true` when the list is taller than the window (a scroll indicator is
    /// meaningful). `false` when everything fits.
    #[must_use]
    pub fn is_scrollable(&self) -> bool {
        self.len > self.viewport
    }

    /// Re-clamp the offset into `0..=max_offset()`. Idempotent; called by
    /// every setter and key transition so the invariant always holds.
    fn clamp(&mut self) {
        self.offset = self.offset.min(self.max_offset());
    }

    /// Set the top-of-window offset, clamped into `0..=max_offset()`.
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset;
        self.clamp();
    }

    /// Replace the backing length (e.g. the list was re-filtered) and
    /// re-clamp the offset so it stays in range.
    pub fn set_len(&mut self, len: usize) {
        self.len = len;
        self.clamp();
    }

    /// Replace the window height (e.g. on resize) and re-clamp the offset.
    pub fn set_viewport(&mut self, viewport: usize) {
        self.viewport = viewport;
        self.clamp();
    }

    /// Ensure row `index` is visible, scrolling the window the minimum amount.
    /// Used by selection-tracking callers (resume/agents) to keep the
    /// highlighted row on screen without owning the scroll math themselves.
    pub fn scroll_to_visible(&mut self, index: usize) {
        if index < self.offset {
            // Above the window — pull the top up to it.
            self.set_offset(index);
        } else if self.viewport > 0 && index >= self.offset + self.viewport {
            // Below the window — push the top down so `index` is the last row.
            self.set_offset(index + 1 - self.viewport);
        }
    }

    /// Apply one scroll key, returning `true` when the key was a scroll key
    /// (handled — even if the offset did not move because it was already at a
    /// clamp). Returns `false` for any other key so the caller can fall
    /// through to its own handling (Enter/Esc/selection/etc.).
    ///
    /// Bindings (claude-code modal pager parity):
    /// - `Up`           → line up   (`offset -= 1`, saturating at 0)
    /// - `Down`         → line down (`offset += 1`, saturating at `max_offset`)
    /// - `PageUp`       → page up   (`offset -= viewport`)
    /// - `PageDown`     → page down (`offset += viewport`)
    /// - `Home`, `Ctrl+Home` → top    (`offset = 0`)
    /// - `End`,  `Ctrl+End`  → bottom (`offset = max_offset`)
    #[must_use]
    pub fn handle_scroll_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up => {
                self.offset = self.offset.saturating_sub(1);
                // Already clamped at the low end; clamp() guards a stale high
                // offset (e.g. after a shrink with no intervening set_len).
                self.clamp();
                true
            }
            KeyCode::Down => {
                self.offset = self.offset.saturating_add(1);
                self.clamp();
                true
            }
            KeyCode::PageUp => {
                self.offset = self.offset.saturating_sub(self.viewport);
                self.clamp();
                true
            }
            KeyCode::PageDown => {
                self.offset = self.offset.saturating_add(self.viewport);
                self.clamp();
                true
            }
            // Bare Home/End and the Ctrl+Home/Ctrl+End emulator bindings both
            // jump to the respective edge.
            KeyCode::Home => {
                self.offset = 0;
                true
            }
            KeyCode::End => {
                self.offset = self.max_offset();
                true
            }
            // Any other key (Enter/Esc/selection/plain chars) is not a scroll
            // key — the caller handles it. Ctrl+Home/Ctrl+End need no special
            // arm: crossterm reports them with the SAME `KeyCode::Home`/`End`
            // as the bare forms (only the modifier differs), so they already
            // hit the Home/End arms above.
            _ => false,
        }
    }
}

/// The visible slice of `rows` for `state`, as a borrowed sub-slice. Returns
/// `rows[offset .. offset+viewport]`, clamped to the list bounds (so a
/// `viewport` taller than the list, or an over-long offset, never panics).
#[must_use]
pub fn visible_slice<'a>(rows: &'a [String], state: &ScrollState) -> &'a [String] {
    let start = state.offset.min(rows.len());
    let end = start.saturating_add(state.viewport).min(rows.len());
    &rows[start..end]
}

/// The visible rows (owned clone) plus an optional scroll-indicator line.
///
/// The indicator is `Some("↓ N more")` / `"↑ N more"` style text only when
/// the list is scrollable AND there is hidden content in that direction; it
/// is `None` when everything fits or the window is at the matching edge.
/// Returns `(visible_rows, indicator)` so the caller renders the slice and,
/// if present, appends the indicator as a dim footer line.
#[must_use]
pub fn visible_window(rows: &[String], state: &ScrollState) -> (Vec<String>, Option<String>) {
    let visible = visible_slice(rows, state).to_vec();
    let indicator = scroll_indicator(state);
    (visible, indicator)
}

/// Build the optional scroll-indicator line for `state`: the count of rows
/// hidden above and/or below the window. `None` when nothing is hidden
/// (list fits, or `viewport == 0`).
///
/// Format (locked): `"↑ {above} more"`, `"↓ {below} more"`, or
/// `"↑ {above} more · ↓ {below} more"` when both directions have hidden rows.
#[must_use]
pub fn scroll_indicator(state: &ScrollState) -> Option<String> {
    if !state.is_scrollable() {
        return None;
    }
    let above = state.offset;
    // Rows after the last visible one: len - (offset + viewport), saturating.
    let below = state
        .len
        .saturating_sub(state.offset.saturating_add(state.viewport));
    match (above, below) {
        (0, 0) => None,
        (a, 0) => Some(format!("\u{2191} {a} more")),
        (0, b) => Some(format!("\u{2193} {b} more")),
        (a, b) => Some(format!("\u{2191} {a} more \u{00B7} \u{2193} {b} more")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn k_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn rows(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("row{i}")).collect()
    }

    #[test]
    fn new_anchors_at_top() {
        let s = ScrollState::new(10, 3);
        assert_eq!(s.offset(), 0);
        assert_eq!(s.len(), 10);
        assert_eq!(s.viewport(), 3);
        assert_eq!(s.max_offset(), 7);
        assert!(s.is_scrollable());
        assert!(!s.is_empty());
    }

    #[test]
    fn max_offset_saturates_when_list_fits() {
        // List shorter than the window: max offset is 0, not scrollable.
        let s = ScrollState::new(2, 5);
        assert_eq!(s.max_offset(), 0);
        assert!(!s.is_scrollable());
        // Exactly fits: still not scrollable, max 0.
        let s = ScrollState::new(5, 5);
        assert_eq!(s.max_offset(), 0);
        assert!(!s.is_scrollable());
    }

    #[test]
    fn down_steps_and_clamps_at_max() {
        let mut s = ScrollState::new(10, 3); // max_offset = 7
        for expected in 1..=7 {
            assert!(s.handle_scroll_key(k(KeyCode::Down)));
            assert_eq!(s.offset(), expected);
        }
        // Past the bottom: stays at max, still reports handled.
        assert!(s.handle_scroll_key(k(KeyCode::Down)));
        assert_eq!(s.offset(), 7);
    }

    #[test]
    fn up_steps_and_clamps_at_zero() {
        let mut s = ScrollState::new(10, 3);
        s.set_offset(5);
        for expected in (0..5).rev() {
            assert!(s.handle_scroll_key(k(KeyCode::Up)));
            assert_eq!(s.offset(), expected);
        }
        // Past the top: stays at 0, still handled.
        assert!(s.handle_scroll_key(k(KeyCode::Up)));
        assert_eq!(s.offset(), 0);
    }

    #[test]
    fn page_down_then_page_up_steps_by_viewport() {
        let mut s = ScrollState::new(20, 5); // max_offset = 15
        assert!(s.handle_scroll_key(k(KeyCode::PageDown)));
        assert_eq!(s.offset(), 5);
        assert!(s.handle_scroll_key(k(KeyCode::PageDown)));
        assert_eq!(s.offset(), 10);
        assert!(s.handle_scroll_key(k(KeyCode::PageUp)));
        assert_eq!(s.offset(), 5);
        // Page up past the top saturates at 0.
        assert!(s.handle_scroll_key(k(KeyCode::PageUp)));
        assert_eq!(s.offset(), 0);
        assert!(s.handle_scroll_key(k(KeyCode::PageUp)));
        assert_eq!(s.offset(), 0);
    }

    #[test]
    fn page_down_clamps_at_max() {
        let mut s = ScrollState::new(20, 5); // max_offset = 15
        for _ in 0..10 {
            assert!(s.handle_scroll_key(k(KeyCode::PageDown)));
        }
        assert_eq!(s.offset(), 15);
    }

    #[test]
    fn home_and_end_jump_to_edges() {
        let mut s = ScrollState::new(30, 4); // max_offset = 26
        assert!(s.handle_scroll_key(k(KeyCode::End)));
        assert_eq!(s.offset(), 26);
        assert!(s.handle_scroll_key(k(KeyCode::Home)));
        assert_eq!(s.offset(), 0);
    }

    #[test]
    fn ctrl_home_and_ctrl_end_jump_to_edges() {
        let mut s = ScrollState::new(30, 4); // max_offset = 26
        assert!(s.handle_scroll_key(k_ctrl(KeyCode::End)));
        assert_eq!(s.offset(), 26);
        assert!(s.handle_scroll_key(k_ctrl(KeyCode::Home)));
        assert_eq!(s.offset(), 0);
    }

    #[test]
    fn non_scroll_key_returns_false_and_does_not_move() {
        let mut s = ScrollState::new(10, 3);
        s.set_offset(4);
        assert!(!s.handle_scroll_key(k(KeyCode::Enter)));
        assert!(!s.handle_scroll_key(k(KeyCode::Esc)));
        assert!(!s.handle_scroll_key(k(KeyCode::Char('q'))));
        // A Ctrl+char that isn't Home/End is also not a scroll key.
        assert!(!s.handle_scroll_key(k_ctrl(KeyCode::Char('c'))));
        assert_eq!(s.offset(), 4);
    }

    #[test]
    fn empty_list_pins_offset_at_zero() {
        let mut s = ScrollState::new(0, 5);
        assert!(s.is_empty());
        assert_eq!(s.max_offset(), 0);
        assert!(s.handle_scroll_key(k(KeyCode::Down)));
        assert_eq!(s.offset(), 0);
        assert!(s.handle_scroll_key(k(KeyCode::End)));
        assert_eq!(s.offset(), 0);
        assert!(s.handle_scroll_key(k(KeyCode::PageDown)));
        assert_eq!(s.offset(), 0);
    }

    #[test]
    fn short_list_never_scrolls() {
        let mut s = ScrollState::new(3, 5); // fits entirely
        assert!(!s.is_scrollable());
        assert!(s.handle_scroll_key(k(KeyCode::Down)));
        assert_eq!(s.offset(), 0);
        assert!(s.handle_scroll_key(k(KeyCode::End)));
        assert_eq!(s.offset(), 0);
    }

    #[test]
    fn set_len_reclamps_offset() {
        let mut s = ScrollState::new(20, 5);
        s.set_offset(15); // at max
        assert_eq!(s.offset(), 15);
        // List shrinks — offset must pull back to the new max (10 - 5 = 5).
        s.set_len(10);
        assert_eq!(s.offset(), 5);
        // Grows back — offset stays where it is (no auto-follow).
        s.set_len(20);
        assert_eq!(s.offset(), 5);
    }

    #[test]
    fn set_viewport_reclamps_offset() {
        let mut s = ScrollState::new(20, 5);
        s.set_offset(15);
        // Taller window → smaller max_offset (20 - 10 = 10).
        s.set_viewport(10);
        assert_eq!(s.offset(), 10);
    }

    #[test]
    fn set_offset_clamps_into_range() {
        let mut s = ScrollState::new(10, 3); // max_offset = 7
        s.set_offset(99);
        assert_eq!(s.offset(), 7);
        s.set_offset(2);
        assert_eq!(s.offset(), 2);
    }

    #[test]
    fn scroll_to_visible_pulls_window_minimally() {
        // Window shows 4 rows.
        let mut s = ScrollState::new(20, 4);
        // Row below the window pushes the top down so it's the last visible:
        // offset becomes 10 + 1 - 4 = 7.
        s.scroll_to_visible(10);
        assert_eq!(s.offset(), 7);
        // Row already visible: no change.
        s.scroll_to_visible(8);
        assert_eq!(s.offset(), 7);
        // Row above the window pulls the top up to it.
        s.scroll_to_visible(2);
        assert_eq!(s.offset(), 2);
    }

    #[test]
    fn visible_slice_returns_the_window() {
        let r = rows(10);
        let mut s = ScrollState::new(10, 3);
        s.set_offset(4);
        let win = visible_slice(&r, &s);
        assert_eq!(
            win,
            &["row4".to_string(), "row5".to_string(), "row6".to_string()]
        );
    }

    #[test]
    fn visible_slice_clamps_when_viewport_exceeds_remaining() {
        let r = rows(5);
        let mut s = ScrollState::new(5, 10); // viewport > len
        s.set_offset(3); // clamped to max_offset 0 anyway
        let win = visible_slice(&r, &s);
        // Whole list, never out of bounds.
        assert_eq!(win.len(), 5);
    }

    #[test]
    fn visible_window_indicator_both_directions() {
        let r = rows(10);
        let mut s = ScrollState::new(10, 3);
        s.set_offset(4); // 4 hidden above, 10-(4+3)=3 hidden below
        let (win, ind) = visible_window(&r, &s);
        assert_eq!(win.len(), 3);
        assert_eq!(
            ind,
            Some("\u{2191} 4 more \u{00B7} \u{2193} 3 more".to_string())
        );
    }

    #[test]
    fn indicator_down_only_at_top() {
        let s = ScrollState::new(10, 3); // offset 0 → only below hidden
        assert_eq!(scroll_indicator(&s), Some("\u{2193} 7 more".to_string()));
    }

    #[test]
    fn indicator_up_only_at_bottom() {
        let mut s = ScrollState::new(10, 3);
        s.set_offset(s.max_offset()); // offset 7 → only above hidden
        assert_eq!(scroll_indicator(&s), Some("\u{2191} 7 more".to_string()));
    }

    #[test]
    fn indicator_none_when_list_fits() {
        let s = ScrollState::new(3, 5);
        assert_eq!(scroll_indicator(&s), None);
        let (_, ind) = visible_window(&rows(3), &s);
        assert_eq!(ind, None);
    }
}
