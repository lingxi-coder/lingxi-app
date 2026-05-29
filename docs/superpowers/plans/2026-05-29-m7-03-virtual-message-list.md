# M7-03 VirtualMessageList — Windowed Scrollback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace M6's capped-500 scrollback buffer with a `VirtualMessageList` that retains the full message log and renders only the lines intersecting the viewport (+ small overscan), backed by a per-message rendered-height cache so scroll math is correct for variable-height messages.

**Architecture:** M6 scrolls in **message rows** (1 message == 1 row, `scroll_offset` and `viewport_height` both counted in messages). That breaks once a message can be 1 line or 200 lines. M7-03 switches the scroll model to **lines**: a `HeightCache` maps `message-index → rendered line count` (recomputed on viewport-width change), `total_lines()` sums it, and a pure `render_window(messages, heights, line_offset, viewport_height)` returns the contiguous slice of messages plus the intra-message top/bottom line offsets to render. The full `Vec<RenderedMessage>` is retained (no eviction). `app::scroll_with_viewport` migrates from message-count clamp math to total-line clamp math; the `j/k/PgUp/PgDn/g/G` keys and the `scroll_started`/`scroll_ended` telemetry emit sites are preserved unchanged in behavior.

**Tech Stack:** Rust 1.82 (pinned via `rust-toolchain.toml` — **run all cargo from inside `lingxi-core/`**), iocraft `=0.8.3` (`View`, not `Box`), `unicode-width` for column-accurate wrap, `insta` 1.40 for snapshots.

---

## File Structure

**Created:**
- `lingxi-core/crates/tui/src/components/virtual_message_list.rs` — the new windowed scrollback component + the pure windowing functions (`HeightCache`, `measured_height`, `render_window`, `WindowSlice`). Replaces `scrollback.rs` as the live component; reuses its per-variant `render_message` dispatch logic.
- `lingxi-core/crates/tui/tests/behavior_virtual_window.rs` — behavior tests: 5k-message render-count gate, mixed-height offset correctness, width-change cache recompute.
- `lingxi-core/crates/tui/tests/render_virtual_window.rs` — insta snapshot of a 3-mixed-message window at a fixed offset.

**Modified:**
- `lingxi-core/crates/tui/src/components/scrollback.rs` — keep the per-variant `render_message` dispatch (moved/shared) but retire the message-row `visible_slice`/`clamp_offset`. To avoid a churny move, `virtual_message_list.rs` will `pub use` the dispatch fn from here; `visible_slice`/`clamp_offset` get `#[deprecated]`-free removal once callers migrate (Task 11).
- `lingxi-core/crates/tui/src/components/mod.rs:9` — register `pub mod virtual_message_list;`.
- `lingxi-core/crates/tui/src/state.rs` — remove the `SCROLLBACK_CAP` eviction from `push_message`; retain the full log. Add a `HeightCache` field + `viewport_width` tracking to `AppState`. Add `RenderedMessage::measured_line_count(width)` helper or route through the free fn.
- `lingxi-core/crates/tui/src/app.rs:483-504` — rewrite `scroll_with_viewport` to clamp against `total_lines` (from the height cache) instead of `messages.len()`. Preserve the `scroll_started`/`scroll_ended` emit sites verbatim.
- `lingxi-core/crates/tui/src/screens/repl.rs:14-15,79-85` — swap `Scrollback` for `VirtualMessageList`; thread the height cache (or recompute inside the component from `messages` + `viewport_width`).
- `lingxi-core/crates/tui/src/root.rs:300,368` — thread `viewport_width` (terminal columns) alongside `viewport_height` into the per-frame render + into the height-cache recompute trigger.

**Read-only references (do NOT edit):**
- `lingxi-core/crates/tui/src/telemetry.rs:52-69` — `scroll_started(offset)` / `scroll_ended()` — must keep firing.
- `lingxi-core/crates/tui/tests/behavior_scroll.rs` — existing scroll behavior tests; must continue passing (they assert offsets that are now line-based — see Task 9 note).
- `lingxi-core/crates/tui/src/components/messages/*.rs` — per-variant renderers (unchanged).

---

## Background: the line-vs-row scroll model migration (read before Task 1)

M6 `scroll_with_viewport` (`app.rs:483`) computes `max = messages.len().saturating_sub(viewport_height)` — i.e. offsets count **messages**. `visible_slice` (`scrollback.rs:69`) slices `&messages[start..end]` by message index. This is correct only when every message is exactly one row tall.

M7-03 makes offsets count **lines**:
- `viewport_height` stays in lines (it already is — `root.rs:viewport_height` returns terminal rows minus 3 chrome rows).
- `scroll_offset` becomes "lines scrolled up from the bottom."
- `max_offset = total_lines.saturating_sub(viewport_height)` where `total_lines = sum of each message's measured height`.
- The window is the contiguous run of messages whose line-ranges intersect `[bottom_line - viewport_height, bottom_line)`, where `bottom_line = total_lines - scroll_offset`.
- The first and last messages in the window may be **partially** visible — `render_window` returns `skip_top_lines` for the first message and `take_lines` budget so the caller renders only the visible lines.

This is the central correctness surface and is exactly what the §4 R3 gate stresses at 5k mixed-height messages.

---

## Task 1: HeightCache type + measured_height pure fn

**Files:**
- Create: `lingxi-core/crates/tui/src/components/virtual_message_list.rs`
- Modify: `lingxi-core/crates/tui/src/components/mod.rs:9`

- [ ] **Step 1: Register the module**

In `lingxi-core/crates/tui/src/components/mod.rs`, after the `pub mod scrollback;` line (line 9), add:

```rust
pub mod virtual_message_list;
```

- [ ] **Step 2: Write the failing test for `measured_height`**

Create `lingxi-core/crates/tui/src/components/virtual_message_list.rs` with this test module at the bottom:

```rust
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
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib virtual_message_list::tests::single_line_message_measures_one`
Expected: FAIL — `cannot find function measured_height`.

- [ ] **Step 4: Implement `measured_height`**

At the top of `virtual_message_list.rs`, above the test module:

```rust
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

use crate::components::scrollback::render_message;
use crate::state::RenderedMessage;

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
        RenderedMessage::UserToolResult { result, .. } => {
            result.as_str().map_or_else(|| result.to_string(), str::to_string)
        }
    }
}
```

> NOTE: `render_message` is referenced now so the `use` compiles; it is made `pub` in Task 2. If the lib does not yet build because `render_message` is private, temporarily inline `let _ = render_message;` is NOT needed — Task 2 runs before any full-crate build is required. To keep Task 1 self-contained, gate the `use crate::components::scrollback::render_message;` import addition to Task 2 and omit it here.

Remove the `render_message` import from Task 1 (it is added in Task 2). Task 1's file only needs `RenderedMessage`, `UnicodeWidthStr`, and the two functions above.

- [ ] **Step 5: Add `unicode-width` dependency if absent**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -q 'unicode-width' crates/tui/Cargo.toml && echo PRESENT || echo MISSING`
If MISSING, add under `[dependencies]` in `crates/tui/Cargo.toml`:

```toml
unicode-width = "=0.1.14"
```

(Exact-pin discipline, matching `Cargo.lock`'s locked `0.1.14` and M7-06's pin — verify with `grep -rn 'unicode-width' Cargo.lock | head -1`. Keeps one version in the tree.)

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib virtual_message_list::tests`
Expected: PASS (4 tests).

- [ ] **Step 7: Commit**

```bash
git add lingxi-core/crates/tui/src/components/virtual_message_list.rs \
        lingxi-core/crates/tui/src/components/mod.rs \
        lingxi-core/crates/tui/Cargo.toml lingxi-core/Cargo.lock
git commit -m "plan(M7-03 T1): add measured_height pure fn + virtual_message_list module

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: HeightCache struct (build / total / per-index lookup)

**Files:**
- Modify: `lingxi-core/crates/tui/src/components/virtual_message_list.rs`
- Modify: `lingxi-core/crates/tui/src/components/scrollback.rs` (make `render_message` `pub`)

- [ ] **Step 1: Make `render_message` reusable**

In `lingxi-core/crates/tui/src/components/scrollback.rs`, change the dispatch fn signature (line 96) from:

```rust
fn render_message(
```

to:

```rust
pub fn render_message(
```

- [ ] **Step 2: Write the failing test for `HeightCache`**

Add to the `tests` module in `virtual_message_list.rs`:

```rust
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
```

- [ ] **Step 3: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib virtual_message_list::tests::height_cache_builds_per_index_and_total`
Expected: FAIL — `cannot find type HeightCache`.

- [ ] **Step 4: Implement `HeightCache`**

Add to `virtual_message_list.rs` (after `measured_height`, and add the `use crate::components::scrollback::render_message;` import at the top now that it is `pub`):

```rust
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
        Self { heights, total, width }
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
```

- [ ] **Step 5: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib virtual_message_list::tests`
Expected: PASS (7 tests).

- [ ] **Step 6: Commit**

```bash
git add lingxi-core/crates/tui/src/components/virtual_message_list.rs \
        lingxi-core/crates/tui/src/components/scrollback.rs
git commit -m "plan(M7-03 T2): HeightCache with build/recompute/total_lines

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: WindowSlice type + render_window core windowing fn (offset 0 / tail)

**Files:**
- Modify: `lingxi-core/crates/tui/src/components/virtual_message_list.rs`

- [ ] **Step 1: Write the failing test for the tail window**

Add to the `tests` module:

```rust
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib virtual_message_list::tests::window_at_offset_zero_shows_tail_lines`
Expected: FAIL — `cannot find function render_window`.

- [ ] **Step 3: Implement `WindowSlice` + `render_window`**

Add to `virtual_message_list.rs`:

```rust
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
        return WindowSlice { empty: true, ..WindowSlice::default() };
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
```

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib virtual_message_list::tests`
Expected: PASS (10 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/virtual_message_list.rs
git commit -m "plan(M7-03 T3): render_window core windowing fn + WindowSlice

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: render_window mixed-height offset correctness

**Files:**
- Modify: `lingxi-core/crates/tui/src/components/virtual_message_list.rs`

- [ ] **Step 1: Write the failing tests for mixed-height offsets**

Add to the `tests` module:

```rust
// Heights: m0=1, m1=50, m2=1, m3=1  → total = 53 lines.
fn mixed_log() -> Vec<RenderedMessage> {
    vec![
        user("m0"),                          // 1 line
        user(&vec!["x"; 50].join("\n")),     // 50 lines
        user("m2"),                          // 1 line
        user("m3"),                          // 1 line
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
```

- [ ] **Step 2: Run to verify pass (logic already implemented in Task 3)**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib virtual_message_list::tests`
Expected: PASS (14 tests). If any mixed-height test FAILS, the off-by-one is in `render_window`'s span intersection — fix the `>`/`>=` boundaries in Task 3's loop until these pass. Do **not** weaken the assertions.

- [ ] **Step 3: Commit**

```bash
git add lingxi-core/crates/tui/src/components/virtual_message_list.rs
git commit -m "plan(M7-03 T4): mixed-height offset correctness tests for render_window

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: AppState migration — retain full log, drop the cap, add height cache

**Files:**
- Modify: `lingxi-core/crates/tui/src/state.rs`

- [ ] **Step 1: Write the failing test for full retention**

Replace the existing `push_evicts_oldest_at_cap` test in `state.rs` (lines 348-363) with:

```rust
#[test]
fn push_retains_full_log_no_eviction() {
    let mut s = AppState::new(fake_status());
    for i in 0..5000 {
        s.push_message(RenderedMessage::UserText {
            body: format!("msg{i}"),
            timestamp: 0,
        });
    }
    // Full retention: every message is kept, in order.
    assert_eq!(s.messages.len(), 5000);
    match &s.messages[0] {
        RenderedMessage::UserText { body, .. } => assert_eq!(body, "msg0"),
        _ => panic!("wrong variant"),
    }
    match &s.messages[4999] {
        RenderedMessage::UserText { body, .. } => assert_eq!(body, "msg4999"),
        _ => panic!("wrong variant"),
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib state::tests::push_retains_full_log_no_eviction`
Expected: FAIL — `assert_eq!(s.messages.len(), 5000)` gets 500.

- [ ] **Step 3: Drop the cap from `push_message`**

In `state.rs`, replace `push_message` (lines 325-331):

```rust
    /// Push a message; evict the oldest if cap exceeded (FIFO).
    pub fn push_message(&mut self, msg: RenderedMessage) {
        self.messages.push(msg);
        if self.messages.len() > SCROLLBACK_CAP {
            self.messages.remove(0);
        }
    }
```

with:

```rust
    /// Push a message. The full log is retained (M7-03 VirtualMessageList
    /// windows the viewport — no FIFO eviction).
    pub fn push_message(&mut self, msg: RenderedMessage) {
        self.messages.push(msg);
    }
```

- [ ] **Step 4: Retire `SCROLLBACK_CAP`**

In `state.rs`, remove the `SCROLLBACK_CAP` const (lines 27-29) and its use in `AppState::new` (line 238). Change:

```rust
            messages: Vec::with_capacity(SCROLLBACK_CAP),
```

to:

```rust
            messages: Vec::new(),
```

Run `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -rn 'SCROLLBACK_CAP' crates/tui/` — there must be **zero** remaining references. Remove any stragglers (e.g. in `scrollback.rs` doc comments).

- [ ] **Step 5: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib state::tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add lingxi-core/crates/tui/src/state.rs lingxi-core/crates/tui/src/components/scrollback.rs
git commit -m "plan(M7-03 T5): retain full message log, drop SCROLLBACK_CAP eviction

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: AppState height-cache + viewport-width fields

**Files:**
- Modify: `lingxi-core/crates/tui/src/state.rs`

- [ ] **Step 1: Write the failing test for the cache field + refresh**

Add to the `tests` module in `state.rs`:

```rust
#[test]
fn height_cache_refresh_tracks_messages_and_width() {
    use crate::components::virtual_message_list::HeightCache;
    let mut s = AppState::new(fake_status());
    s.push_message(RenderedMessage::UserText { body: "a\nb".into(), timestamp: 0 });
    s.push_message(RenderedMessage::UserText { body: "c".into(), timestamp: 0 });
    s.refresh_height_cache(80);
    assert_eq!(s.height_cache.total_lines(), 3); // 2 + 1
    assert_eq!(s.viewport_width, 80);
    // Width change recomputes.
    s.push_message(RenderedMessage::UserText { body: "x".repeat(20), timestamp: 0 });
    s.refresh_height_cache(10); // "xxxxxxxxxxxxxxxxxxxx" → ceil(20/10)=2
    assert_eq!(s.height_cache.total_lines(), 5); // 2 + 1 + 2
    let _ = HeightCache::default(); // type is reachable
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib state::tests::height_cache_refresh_tracks_messages_and_width`
Expected: FAIL — `no field height_cache on AppState`.

- [ ] **Step 3: Add the fields + refresh method**

In `state.rs`, add to the `AppState` struct (after the `scroll_offset` field, ~line 198):

```rust
    /// (M7-03) Per-message rendered-height cache backing line-based scroll
    /// math. Rebuilt by [`Self::refresh_height_cache`] when the log grows
    /// or the viewport width changes.
    pub height_cache: crate::components::virtual_message_list::HeightCache,
    /// (M7-03) Viewport width (terminal columns) the cache was last built
    /// for. A change here triggers a recompute.
    pub viewport_width: usize,
```

In `AppState::new` (the struct literal, ~line 246, after `scroll_offset: 0,`):

```rust
            height_cache: crate::components::virtual_message_list::HeightCache::default(),
            viewport_width: 0,
```

Add the refresh method to the `impl AppState` block (next to `push_message`):

```rust
    /// (M7-03) Rebuild the height cache if the log or `width` changed.
    /// Idempotent: a no-op when nothing changed (cheap len + width check).
    pub fn refresh_height_cache(&mut self, width: usize) {
        let stale = self.viewport_width != width
            || self.height_cache.len() != self.messages.len();
        if stale {
            self.height_cache
                .recompute(&self.messages, width);
            self.viewport_width = width;
        }
    }
```

> NOTE on the staleness check: comparing `len()` catches the common append-only case. A message whose *content* mutates in place (none do in M7 — `RenderedMessage` is push-only) would not be caught; that is acceptable for M7-03. M7-04/05 streaming renderers that mutate the last message must call `refresh_height_cache` after forcing a width bump or add a dirty flag (out of scope here; note it).

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib state::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/state.rs
git commit -m "plan(M7-03 T6): AppState gains height_cache + viewport_width + refresh

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Migrate scroll_with_viewport to line-based clamp math

**Files:**
- Modify: `lingxi-core/crates/tui/src/app.rs:483-504`

- [ ] **Step 1: Write the failing line-based scroll test**

Create `lingxi-core/crates/tui/tests/behavior_virtual_window.rs`:

```rust
//! M7-03 behavior tests: line-based scroll, windowing, telemetry, cache.

use lingxi_tui::app::scroll_with_viewport;
use lingxi_tui::events::keymap::ScrollDir;
use lingxi_tui::state::{AppState, RenderedMessage};

mod support;
use support::fake_status;

fn push_line(st: &mut AppState, body: &str) {
    st.push_message(RenderedMessage::UserText {
        body: body.to_string(),
        timestamp: 0,
    });
}

#[test]
fn line_step_uses_total_lines_not_message_count() {
    let mut st = AppState::new(fake_status());
    // One 100-line message → total_lines = 100, message count = 1.
    push_line(&mut st, &vec!["x"; 100].join("\n"));
    let vh = 10;
    st.refresh_height_cache(80);
    // LineUp scrolls one line; max_offset = 100 - 10 = 90.
    scroll_with_viewport(&mut st, ScrollDir::LineUp, vh);
    assert_eq!(st.scroll_offset, 1);
    // Top pins to max_offset = 90 (lines), NOT 0 (messages.len - vh).
    scroll_with_viewport(&mut st, ScrollDir::Top, vh);
    assert_eq!(st.scroll_offset, 90);
    // Bottom returns to 0.
    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}

#[test]
fn pgup_pgdn_step_by_viewport_lines() {
    let mut st = AppState::new(fake_status());
    push_line(&mut st, &vec!["x"; 100].join("\n"));
    st.refresh_height_cache(80);
    let vh = 8;
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    assert_eq!(st.scroll_offset, 16);
    scroll_with_viewport(&mut st, ScrollDir::PageDown, vh);
    assert_eq!(st.scroll_offset, 8);
}
```

- [ ] **Step 2: Add the `support` shim if behavior tests share one**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && ls crates/tui/tests/support* crates/tui/tests/support/ 2>/dev/null`
The existing `behavior_scroll.rs` already uses `mod support; use support::fake_status;` — reuse the same `crates/tui/tests/support.rs` (or `support/mod.rs`). No new shim needed.

- [ ] **Step 3: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test behavior_virtual_window line_step_uses_total_lines_not_message_count`
Expected: FAIL — `Top` pins to 0 (old message-count math gives `max = 1 - 10 → 0`).

- [ ] **Step 4: Rewrite `scroll_with_viewport`**

In `app.rs`, replace the body of `scroll_with_viewport` (lines 483-504). Keep the signature and the two telemetry emit sites verbatim:

```rust
pub fn scroll_with_viewport(st: &mut AppState, dir: ScrollDir, viewport_height: usize) {
    // (M7-03) Scroll math is LINE-based: clamp against total rendered lines
    // from the height cache, not the message count. Refresh the cache at
    // the current viewport width first so `total_lines` is accurate.
    st.refresh_height_cache(st.viewport_width.max(1));
    let total = st.height_cache.total_lines();
    let max = total.saturating_sub(viewport_height) as i64;
    let cur = st.scroll_offset as i64;
    let new = match dir {
        ScrollDir::LineUp => cur + 1,
        ScrollDir::LineDown => cur - 1,
        ScrollDir::PageUp => cur + viewport_height as i64,
        ScrollDir::PageDown => cur - viewport_height as i64,
        ScrollDir::Top => max,
        ScrollDir::Bottom => 0,
    };
    let prev_offset = st.scroll_offset;
    st.scroll_offset = new.clamp(0, max) as usize;
    // (M6-09) Emit scroll-mode lifecycle on the offset transition: 0 →
    // non-zero opens scroll mode; non-zero → 0 closes it (back at bottom).
    if prev_offset == 0 && st.scroll_offset != 0 {
        crate::telemetry::scroll_started(st.scroll_offset);
    } else if prev_offset != 0 && st.scroll_offset == 0 {
        crate::telemetry::scroll_ended();
    }
}
```

> NOTE: `st.viewport_width` is 0 until the first frame threads it (Task 8). `.max(1)` keeps the cache build safe before then. The tests call `refresh_height_cache(80)` explicitly, setting `viewport_width = 80`, so `scroll_with_viewport`'s internal refresh re-uses width 80.

- [ ] **Step 5: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test behavior_virtual_window`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add lingxi-core/crates/tui/src/app.rs lingxi-core/crates/tui/tests/behavior_virtual_window.rs
git commit -m "plan(M7-03 T7): scroll_with_viewport clamps against total_lines

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: VirtualMessageList component + thread viewport_width through render

**Files:**
- Modify: `lingxi-core/crates/tui/src/components/virtual_message_list.rs`
- Modify: `lingxi-core/crates/tui/src/screens/repl.rs:14-15,79-85`
- Modify: `lingxi-core/crates/tui/src/root.rs:300,368`

- [ ] **Step 1: Implement the `VirtualMessageList` component**

Add to `virtual_message_list.rs`:

```rust
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
```

> NOTE: iocraft clips overflow at the `View` boundary, so rendering whole messages for the first/last partially-visible rows is acceptable — `skip_top_lines`/`take_lines` inform the offset math and future per-line clipping (M7-04+), but the line-flex container visually clips the overscan. The **render count** (number of `render_message` calls) is bounded by the window size, which is the §4 R3 gate.

- [ ] **Step 2: Swap the component in `repl.rs`**

In `repl.rs`, change the import (line 15):

```rust
use crate::components::scrollback::Scrollback;
```

to:

```rust
use crate::components::virtual_message_list::VirtualMessageList;
```

Add a `viewport_width` field to `ReplScreenProps` (after `viewport_height`, line 44):

```rust
    /// Live viewport width (columns) for the height cache.
    pub viewport_width: usize,
```

Bind it in the component body (after `let viewport_height = props.viewport_height;`, line 66):

```rust
    let viewport_width = props.viewport_width;
```

Replace the `Scrollback(...)` mount (lines 79-85) with:

```rust
            VirtualMessageList(
                messages: messages,
                scroll_offset: scroll_offset,
                viewport_height: viewport_height,
                viewport_width: viewport_width,
                expanded: expanded,
                focused_tool_id: focused_tool_id,
            )
```

- [ ] **Step 3: Thread `viewport_width` from `root.rs`**

In `root.rs`, at both render call sites (lines ~300 and ~368), the `ReplScreen` element is built via `app::render_app` / a `ReplScreen(...)` invocation that already passes `viewport_height`. Find where `viewport` is computed (`let viewport = viewport_height(rows);`) and add the column count.

Add a `viewport_width` helper near `viewport_height` (`root.rs:401`):

```rust
/// Columns available to the scrollback. The REPL reserves no horizontal
/// chrome today, so this is the full terminal width (min 1).
fn viewport_width(cols: u16) -> usize {
    (cols as usize).max(1)
}
```

At each render site, capture the terminal columns (the same place `rows` is read — iocraft exposes width alongside height; if the frame only has `rows`, read `cols` from the same terminal-size source). Pass `viewport_width: viewport_width(cols)` into the `ReplScreen(...)` props, and call `state.refresh_height_cache(viewport_width(cols))` before reading `scroll_offset` so the cache matches the width the component will build at.

If `render_app` in `app.rs` (the `ReplScreen(...)` builder around line 317-329) is the single render path, add the prop there and pass `viewport_width` down from `root.rs` through the same parameter that carries `viewport_height`.

- [ ] **Step 4: Write the failing test — render count bounded by viewport**

Add to `virtual_message_list.rs` tests:

```rust
#[test]
fn window_render_count_bounded_by_viewport_not_log_size() {
    // 5000 single-line messages, viewport 20 → window holds ~20 (+overscan),
    // never 5000.
    let msgs: Vec<RenderedMessage> = (0..5000).map(|i| user(&format!("m{i}"))).collect();
    let cache = HeightCache::build(&msgs, 80);
    assert_eq!(cache.total_lines(), 5000);
    let win = render_window(&msgs, &cache, 0, 20);
    let count = win.indices().count();
    assert!(count <= 21, "window rendered {count} messages, expected <= 21");
    assert!(count >= 20, "window should fill the viewport, got {count}");
}
```

- [ ] **Step 5: Run to verify pass + build**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib virtual_message_list::tests && cargo build -p lingxi-tui`
Expected: PASS; crate builds clean (component + repl + root wiring compile).

- [ ] **Step 6: Commit**

```bash
git add lingxi-core/crates/tui/src/components/virtual_message_list.rs \
        lingxi-core/crates/tui/src/screens/repl.rs \
        lingxi-core/crates/tui/src/root.rs \
        lingxi-core/crates/tui/src/app.rs
git commit -m "plan(M7-03 T8): VirtualMessageList component + thread viewport_width

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: Preserve j/k/PgUp/PgDn/g/G behavior + migrate the M6 scroll tests

**Files:**
- Modify: `lingxi-core/crates/tui/tests/behavior_scroll.rs`
- Read: `lingxi-core/crates/tui/src/events/keymap.rs` (key→ScrollDir mapping; unchanged)

- [ ] **Step 1: Understand why the M6 tests change**

`behavior_scroll.rs` builds 30 one-line messages and asserts `scroll_offset == vh*2`, `max == 22`, etc. Under the **old** message-count math those numbers came from `messages.len()`. Under the **new** line-based math, 30 one-line messages still total 30 lines, so the same numbers hold **only if the height cache is refreshed at the test width first**. The keymap (`j/k/PgUp/PgDn/g/G → ScrollDir`) does not change.

- [ ] **Step 2: Update `behavior_scroll.rs` `buf_30` to refresh the cache**

In `behavior_scroll.rs`, change `buf_30` to refresh the height cache after pushing:

```rust
fn buf_30() -> AppState {
    let mut st = AppState::new(fake_status());
    for i in 0..30 {
        st.push_message(RenderedMessage::AssistantText {
            body: format!("a{i}"),
            timestamp: 0,
        });
    }
    st.refresh_height_cache(80); // line-based scroll needs the cache populated
    st
}
```

The three existing assertions (`vh*2`, `max == 22` for vh=8, `25`/`0` for vh=5) remain correct because 30 one-line messages = 30 lines.

- [ ] **Step 3: Add an explicit j/k/g/G assertion to the new behavior file**

Add to `behavior_virtual_window.rs`:

```rust
#[test]
fn jk_gg_keys_move_offset_line_based() {
    let mut st = AppState::new(fake_status());
    for i in 0..40 {
        push_line(&mut st, &format!("l{i}")); // 40 one-line msgs = 40 lines
    }
    st.refresh_height_cache(80);
    let vh = 10;
    // j == LineUp (older), k == LineDown (newer) per keymap.
    scroll_with_viewport(&mut st, ScrollDir::LineUp, vh);
    assert_eq!(st.scroll_offset, 1);
    scroll_with_viewport(&mut st, ScrollDir::LineDown, vh);
    assert_eq!(st.scroll_offset, 0);
    // g == Top → max_offset = 40 - 10 = 30. G == Bottom → 0.
    scroll_with_viewport(&mut st, ScrollDir::Top, vh);
    assert_eq!(st.scroll_offset, 30);
    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}
```

> NOTE: confirm the `j`/`k` → `LineUp`/`LineDown` mapping in `keymap.rs:137-140` before asserting (M6 maps `j`→`LineDown`? read the file). If `j` is mapped to `LineDown` (scroll toward newer/bottom) adjust the direction in the assert to match the actual keymap — do not change the keymap.

- [ ] **Step 4: Run both behavior files**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test behavior_scroll --test behavior_virtual_window`
Expected: PASS (all).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/tests/behavior_scroll.rs \
        lingxi-core/crates/tui/tests/behavior_virtual_window.rs
git commit -m "plan(M7-03 T9): preserve j/k/PgUp/PgDn/g/G under line-based scroll

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 10: Scroll telemetry still fires on 0↔nonzero transition

**Files:**
- Modify: `lingxi-core/crates/tui/tests/behavior_virtual_window.rs`
- Read: `lingxi-core/crates/tui/src/telemetry.rs:52-69`

- [ ] **Step 1: Find the telemetry capture pattern used by M6**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -rln 'tracing_subscriber\|with_capture\|TestWriter\|tracing_test\|SCROLL_STARTED\|SCROLL_ENDED' crates/tui/tests/`
Use the same capture harness the M6 scroll-telemetry behavior test uses (a `tracing` subscriber that records `event` fields). If M6 has no telemetry-capture test, capture via a `tracing_subscriber::fmt` layer writing to a shared `Vec<u8>` buffer and assert the event-name constants appear.

- [ ] **Step 2: Write the telemetry transition test**

Add to `behavior_virtual_window.rs`. This asserts the **state transition** that drives the emit (offset 0 → nonzero → 0); pair it with whatever capture harness step 1 found. If no harness exists, assert behaviorally on the offset transitions that gate the emit sites (the emit is unconditional given the transition, verified by the source at `app.rs:499-503`):

```rust
#[test]
fn scroll_telemetry_transitions_fire_on_0_to_nonzero_and_back() {
    let mut st = AppState::new(fake_status());
    for i in 0..40 {
        push_line(&mut st, &format!("t{i}"));
    }
    st.refresh_height_cache(80);
    let vh = 10;
    assert_eq!(st.scroll_offset, 0); // start at bottom

    // 0 → non-zero: scroll_started fires (offset becomes 10).
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    assert_ne!(st.scroll_offset, 0);

    // non-zero → non-zero: no lifecycle emit (still scrolling).
    scroll_with_viewport(&mut st, ScrollDir::LineUp, vh);
    assert_ne!(st.scroll_offset, 0);

    // non-zero → 0: scroll_ended fires (back at bottom).
    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}
```

If step 1 found a capture harness, additionally assert `SCROLL_STARTED` appears exactly once and `SCROLL_ENDED` appears exactly once in the captured events across the three calls.

- [ ] **Step 2b: Confirm emit sites are unchanged**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -n 'scroll_started\|scroll_ended' crates/tui/src/app.rs`
Expected: the two emit calls at the offset-transition branches still exist verbatim (Task 7 preserved them). The telemetry baseline stays at 326 — M7-03 adds **zero** new events.

- [ ] **Step 3: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test behavior_virtual_window scroll_telemetry`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add lingxi-core/crates/tui/tests/behavior_virtual_window.rs
git commit -m "plan(M7-03 T10): scroll telemetry still fires on 0<->nonzero transition

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 11: Retire the message-row scrollback API + repoint imports

**Files:**
- Modify: `lingxi-core/crates/tui/src/components/scrollback.rs`
- Modify: any caller of `visible_slice` / `clamp_offset` / `Scrollback`

- [ ] **Step 1: Find remaining callers of the old API**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -rn 'visible_slice\|clamp_offset\|::Scrollback\|Scrollback(' crates/tui/`
Catalog every hit. `repl.rs` was repointed in Task 8. Snapshot tests referencing `Scrollback` directly (if any) get repointed to `VirtualMessageList`.

- [ ] **Step 2: Decide keep-vs-remove for `render_message`**

`render_message` stays in `scrollback.rs` (now `pub`, re-used by `VirtualMessageList`). Remove `visible_slice`, `clamp_offset`, the `Scrollback` component, `ScrollbackProps`, and their tests — they encode the obsolete message-row model. Keep the file as the home of `render_message` + its variant dispatch, and update the module doc-comment to "per-variant message dispatch (windowing lives in `virtual_message_list`)."

- [ ] **Step 3: Apply the removals**

Delete from `scrollback.rs`: the `Scrollback` component (lines 46-62), `ScrollbackProps` (lines 29-44), `visible_slice` (lines 64-81), `clamp_offset` (lines 83-94), and the `tests` module assertions for `visible_slice`/`clamp_offset` (the whole `#[cfg(test)] mod tests` block at lines 154-201). Keep `render_message` and its imports.

- [ ] **Step 4: Repoint any stray imports**

For each hit from Step 1 outside `repl.rs`/`virtual_message_list.rs`, change `use crate::components::scrollback::{Scrollback, ...}` to import from `virtual_message_list` (component) or keep `scrollback::render_message` (dispatch). Run the grep again — zero references to the removed items must remain.

- [ ] **Step 5: Run the crate test + clippy**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui && cargo clippy -p lingxi-tui --all-targets -- -D warnings`
Expected: PASS; no clippy warnings (no dead-code warnings from the removals).

- [ ] **Step 6: Commit**

```bash
git add lingxi-core/crates/tui/src/components/scrollback.rs lingxi-core/crates/tui/
git commit -m "plan(M7-03 T11): retire message-row scrollback API, keep render_message dispatch

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 12: Snapshot — a window of 3 mixed-height messages at a fixed offset

**Files:**
- Create: `lingxi-core/crates/tui/tests/render_virtual_window.rs`

- [ ] **Step 1: Write the snapshot test**

Create `lingxi-core/crates/tui/tests/render_virtual_window.rs`:

```rust
//! M7-03 snapshot: a window of 3 mixed-height messages at a fixed offset.

use std::collections::HashMap;

use iocraft::prelude::*;
use lingxi_tui::components::virtual_message_list::VirtualMessageList;
use lingxi_tui::state::RenderedMessage;

#[test]
fn window_three_mixed_messages_fixed_offset() {
    let messages = vec![
        RenderedMessage::UserText {
            body: "short user line".into(),
            timestamp: 0,
        },
        RenderedMessage::AssistantText {
            body: "first\nsecond\nthird\nfourth".into(), // 4 lines
            timestamp: 0,
        },
        RenderedMessage::SystemText {
            body: "system note".into(),
            timestamp: 0,
            is_error: false,
        },
    ];
    let mut element = element! {
        VirtualMessageList(
            messages: messages,
            scroll_offset: 0_usize,
            viewport_height: 8_usize,
            viewport_width: 40_usize,
            expanded: HashMap::new(),
            focused_tool_id: Option::<lingxi_protocol::ToolUseId>::None,
        )
    };
    insta::assert_snapshot!("window_three_mixed_fixed_offset", element.to_string());
}
```

- [ ] **Step 2: Run to generate the snapshot**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test render_virtual_window`
Expected: FAIL — new snapshot pending review.

- [ ] **Step 3: Review and accept the snapshot**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo insta review` (or inspect `crates/tui/tests/snapshots/render_virtual_window__window_three_mixed_fixed_offset.snap.new` and confirm all three messages render in order with the assistant's 4 lines intact). Accept: `cargo insta accept`.

- [ ] **Step 4: Re-run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test render_virtual_window`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/tests/render_virtual_window.rs \
        lingxi-core/crates/tui/tests/snapshots/render_virtual_window__window_three_mixed_fixed_offset.snap
git commit -m "plan(M7-03 T12): snapshot a window of 3 mixed-height messages

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 13: THE 5K GATE (§4 R3) — mixed-height correctness + bounded render at scale

**Files:**
- Modify: `lingxi-core/crates/tui/tests/behavior_virtual_window.rs`

This is the spec §4 R3 hard gate. It must pass before the tag. If windowing proves unstable, apply the GATE FALLBACK below.

- [ ] **Step 1: Write the 5k mixed-height gate test**

Add to `behavior_virtual_window.rs`:

```rust
use lingxi_tui::components::virtual_message_list::{render_window, HeightCache};

/// Build 5000 mixed-height messages: every 10th is 50 lines tall, the
/// rest are 1 line. Returns (messages, total_lines).
fn mixed_5k() -> (Vec<RenderedMessage>, usize) {
    let msgs: Vec<RenderedMessage> = (0..5000)
        .map(|i| {
            let body = if i % 10 == 0 {
                vec!["x"; 50].join("\n") // 50 lines
            } else {
                format!("m{i}") // 1 line
            };
            RenderedMessage::UserText { body, timestamp: 0 }
        })
        .collect();
    // 500 tall (×50) + 4500 short (×1) = 25_000 + 4_500 = 29_500 lines.
    (msgs, 29_500)
}

#[test]
fn gate_5k_render_count_bounded_by_viewport() {
    let (msgs, total) = mixed_5k();
    let cache = HeightCache::build(&msgs, 80);
    assert_eq!(cache.total_lines(), total);
    let vh = 30;
    // At the bottom (offset 0) the window must be a tiny slice, never 5000.
    let win = render_window(&msgs, &cache, 0, vh);
    let count = win.indices().count();
    assert!(count < 100, "render count {count} not bounded by viewport");
    // The last visible message is the final one (offset 0 = tail).
    assert_eq!(win.last_index, 4999);
}

#[test]
fn gate_5k_exact_first_last_visible_at_offset() {
    let (msgs, total) = mixed_5k();
    let cache = HeightCache::build(&msgs, 80);
    assert_eq!(total, 29_500);
    let vh = 20;
    // Scroll so the viewport sits entirely inside the tall message at
    // index 2490 (i % 10 == 0). Compute its absolute start line:
    //   short msgs before any tall one... derive the start of msg 2490.
    let mut start = 0usize;
    for i in 0..2490 {
        start += cache.height_at(i);
    }
    // msg 2490 spans [start, start+50). Put the viewport at [start+10, start+30).
    // bottom_line = start + 30  ⇒  offset = total - bottom_line.
    let bottom_line = start + 30;
    let offset = total - bottom_line;
    let win = render_window(&msgs, &cache, offset, vh);
    // Entirely inside one tall message.
    assert_eq!(win.first_index, 2490);
    assert_eq!(win.last_index, 2490);
    assert_eq!(win.skip_top_lines, 10); // top_line - span_start = (start+10) - start
}

#[test]
fn gate_5k_offset_zero_first_visible_is_correct() {
    let (msgs, total) = mixed_5k();
    let cache = HeightCache::build(&msgs, 80);
    let vh = 30;
    // offset 0 → top_line = total - 30. Tail messages 4970..4999 are all
    // 1-line except 4990 (i%10==0, 50 lines). Last 30 lines walk back:
    //   4999..4991 = 9 one-line msgs (9 lines), 4990 = 50 lines.
    // 9 + (need 21 more lines from msg 4990's 50) → first_index = 4990.
    let win = render_window(&msgs, &cache, 0, vh);
    assert_eq!(win.last_index, 4999);
    assert_eq!(win.first_index, 4990);
    // 50-line msg 4990 spans [total-9-50, total-9). top_line = total-30.
    // skip_top_lines = top_line - span_start = (total-30) - (total-59) = 29.
    assert_eq!(win.skip_top_lines, 29);
}
```

- [ ] **Step 2: Run the gate**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test behavior_virtual_window gate_5k`
Expected: PASS (3 gate tests). If an exact-index assertion is off by one, the bug is in `render_window`'s span-intersection boundaries (`span_end > top_line && span_start < bottom_line`) — fix the windowing fn, never weaken the gate assertions.

- [ ] **Step 3: GATE FALLBACK (only if windowing proves unstable)**

> **GATE FALLBACK (spec §4 R3):** If after honest debugging the windowing offset math cannot be made correct at 5k mixed-height messages (persistent off-by-one that resists the fix, or a structural iocraft clipping problem), do NOT ship broken windowing. Instead apply the **documented degrade**:
>
> 1. Reintroduce a cap constant but raise it far above M6's 500 — e.g. `pub const SCROLLBACK_CAP: usize = 5000;` in `state.rs`, with FIFO eviction restored in `push_message`.
> 2. Render-all within that cap (keep the M6-style message-row `visible_slice`, or render the whole capped log) with a **perf ceiling**: a behavior test asserting render of a 5000-message log completes under a fixed budget (e.g. window construction < 50ms; documented as a soft budget per spec §5.7).
> 3. Keep `render_window` + `HeightCache` in the tree behind the cap (they remain the forward path); document in the `virtual_message_list.rs` module doc that windowing is gated-off pending an M8 fix, and file the residual off-by-one as an M8 follow-up.
> 4. The milestone is NOT blocked — this degrades scope (windowing → raised-cap render-all), not schedule, exactly as §4 states.
>
> Default expectation: the windowing math is correct and this fallback is NOT taken. The fallback exists so the gate degrades gracefully rather than blocking the tag.

- [ ] **Step 4: Commit**

```bash
git add lingxi-core/crates/tui/tests/behavior_virtual_window.rs
git commit -m "plan(M7-03 T13): 5k mixed-height gate — bounded render + exact first/last visible

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 14: Workspace verification gate + tag m7.3

**Files:** none (verification + tag only)

- [ ] **Step 1: Format check (from inside `lingxi-core/`)**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo fmt --check`
Expected: clean (no diff). If it reports files, run `cargo fmt` and amend nothing — instead fix and continue.

- [ ] **Step 2: Clippy (workspace, deny warnings)**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. The cast lints on `scroll_with_viewport` keep the existing `#[allow(...)]` attributes — do not remove them.

- [ ] **Step 3: Full workspace test**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test --workspace`
Expected: PASS. Known flakes (rerun-allowed, NOT failures): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing tests. If any of these flake, rerun that single test once to confirm green.

- [ ] **Step 4: Cross-platform compile gate (5 targets)**

Run:
```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && \
for t in x86_64-unknown-linux-gnu x86_64-apple-darwin x86_64-pc-windows-gnu aarch64-linux-android aarch64-apple-ios; do \
  echo "=== $t ===" && cargo check --workspace --target "$t" || break; \
done
```
Expected: all 5 green (same posture as v0.7.0). If a target's toolchain isn't installed, install via `rustup target add <t>` first.

- [ ] **Step 5: Confirm telemetry baseline unchanged (still 326)**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test --workspace -- ALL_EVENT_NAMES 2>/dev/null; grep -rn 'ALL_EVENT_NAMES' crates/ | head`
Expected: the event-name count test (wherever it lives) still passes at 326 — M7-03 added zero events. If the count moved, a new event leaked in; remove it (M7-03 reuses `scroll_started`/`scroll_ended` only).

- [ ] **Step 6: Tag m7.3**

```bash
git tag -a m7.3 -m "M7-03: VirtualMessageList — windowed scrollback (line-based, 5k gate)"
```

> Per spec §6.4: tags are **local only**; NO remote push from Claude. No force-push, no `--no-verify`, no amends.

- [ ] **Step 7: Confirm the tag**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next && git tag --list 'm7.3' && git log -1 --oneline m7.3`
Expected: `m7.3` lists; points at the T13 (or T14 verification) commit.

---

## Self-Review

**Spec coverage (against §1 M7-03 deliverables, §2.3 data-flow, §4 R3, §3 M7-03 entry):**
- "windowed scrollback replacing M6's capped-500 buffer" → Tasks 5 (drop cap), 8 (VirtualMessageList component). ✓
- "per-message rendered-height cache; recompute on width change" → Tasks 2, 6 (`HeightCache`, `refresh_height_cache`, width-change recompute test). ✓
- "`render_window(messages, offset, viewport_height)` core windowing fn returning slice + intra-message line offset" → Task 3 (`render_window` + `WindowSlice{skip_top_lines, take_lines}`). ✓
- "preserve j/k/PgUp/PgDn/g/G; scroll telemetry emit sites keep working" → Tasks 7, 9, 10. ✓
- "AppState migrates from Vec cap to full retention" → Task 5. ✓
- "GATE: correct mixed-height rendering at 5k, else degrade per §4 R3" → Task 13 + GATE FALLBACK note. ✓
- Tests required (5k bound, mixed-height offset, j/k/PgUp/PgDn/g/G, cache recompute on width, telemetry transition, snapshot of 3 mixed) → Tasks 13, 4, 9, 2/6, 10, 12. ✓ All six present.
- Telemetry: 0 new events; baseline 326 confirmed → Task 14 Step 5. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to Task N". Every code step shows full code. The Task 1 NOTE removes a premature `use` to keep the task self-building; Task 2 adds it with the `pub` change — consistent.

**Type consistency:** `HeightCache` (build/recompute/height_at/total_lines/len/is_empty/width), `WindowSlice` (first_index/last_index/skip_top_lines/take_lines/empty + indices()/is_empty()), `render_window`, `measured_height`, `OVERSCAN_LINES`, `VirtualMessageList`/`VirtualMessageListProps`, `AppState.height_cache`/`viewport_width`/`refresh_height_cache` — names used identically across Tasks 1-13. `scroll_with_viewport` signature unchanged (Task 7 keeps it). `render_message` made `pub` in Task 2, reused in Task 8.

**Known risk flagged:** Task 9 Step 3 NOTE — verify the actual `j`/`k` → `ScrollDir` mapping in `keymap.rs` before asserting direction. Task 8 Step 3 — `root.rs` column source must be confirmed (iocraft frame width); if `render_app` is the single render path, the prop threads there. These are read-then-adapt steps, not placeholders.
