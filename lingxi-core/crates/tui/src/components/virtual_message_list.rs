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
///
/// # Invariant: this MUST track [`render_message`]'s actual output
/// The whole scroll model is line-based, so the height this returns drives
/// `scroll_offset`/window math. It is computed from the text proxy in
/// [`render_text_for_measure`], which reconstructs per-variant text rather
/// than reading what [`render_message`] actually draws. The two can DRIFT:
/// if a renderer changes how a variant is laid out (extra prefix rows,
/// truncation, expanded JSON, a multi-line diff body) without the proxy
/// being updated to match, the scroll math desyncs from the rendered
/// output (rows skipped/duplicated at the viewport edges).
///
/// Therefore: **any change to a `render_message` variant's line layout — or
/// any new [`RenderedMessage`] variant — REQUIRES updating
/// [`render_text_for_measure`] to match, and updating the
/// `measured_height_pins_*` lock tests in this module.** The richer
/// per-variant renderers in M7-04/05 will eventually unify measurement with
/// rendering; until then this proxy + its lock tests are the guardrail.
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
///
/// # MUST stay in lock-step with [`render_message`]
/// This is a measurement *proxy*: it reconstructs the text each variant
/// renders so [`measured_height`] can count rows without a terminal. It is
/// NOT derived from the real render path (which yields an iocraft element,
/// not text), so it can silently drift from what `render_message` draws.
/// When you add a [`RenderedMessage`] variant, or change how an existing
/// variant lays out rows in `render_message` / its per-variant component,
/// you MUST update this function to match and extend the
/// `measured_height_pins_*` lock tests below. Drift here corrupts the
/// line-based scroll math.
#[allow(clippy::too_many_lines)] // one arm per RenderedMessage variant (15 variants)
fn render_text_for_measure(msg: &RenderedMessage) -> String {
    match msg {
        RenderedMessage::UserText { body, .. }
        | RenderedMessage::AssistantText { body, .. }
        | RenderedMessage::SystemText { body, .. } => body.clone(),
        RenderedMessage::AssistantToolUse { tool, .. } => format!("● {tool}(…)"),
        RenderedMessage::UserToolResult { result, .. } => result
            .as_str()
            .map_or_else(|| result.to_string(), str::to_string),
        // ---- (M7-04) batch-1 system/assistant renderers ----------------
        // Each arm reproduces the line layout that `render_message` draws for
        // the variant (via its `render_*_to_string` pure renderer). Markers
        // (`∴ `/`✻ `/`● `) add columns, not rows. Kept in lock-step with the
        // renderers in `components::messages::*`; pinned by the
        // `measured_height_pins_*_m7_04` lock tests below.
        // Measurement == render by construction: route through the renderer's
        // own string oracle so the expanded body counts the SAME
        // markdown-FLATTENED lines the component draws (the raw `thinking`
        // text over-counts dropped ``` fence rows / trailing blanks). Covers
        // the collapsed header+hint line too.
        RenderedMessage::AssistantThinking { thinking, expanded } => {
            crate::components::messages::thinking::render_thinking_to_string(
                crate::components::messages::thinking::ThinkingProps {
                    thinking: thinking.clone(),
                    expanded: *expanded,
                },
            )
        }
        // Single dim+italic line `✻ Thinking…`.
        RenderedMessage::AssistantRedactedThinking => "\u{273B} Thinking\u{2026}".to_string(),
        // Single dim boundary line (counts not rendered — claude-code parity).
        RenderedMessage::CompactBoundary { .. } => {
            "\u{273B} Conversation compacted (ctrl+o for history)".to_string()
        }
        // Info → body verbatim; warning/error → `● ` marker (cols) + body.
        RenderedMessage::SystemTextRich { body, level } => match level {
            crate::state::SystemLevel::Info => body.clone(),
            crate::state::SystemLevel::Warning | crate::state::SystemLevel::Error => {
                format!("\u{25CF} {body}")
            }
        },
        // error body (+ optional `…` + expand-hint line when truncated) then a
        // retry-countdown footer line.
        RenderedMessage::SystemApiError {
            error,
            retry_attempt,
            retry_in_seconds,
            max_retries,
            truncated,
        } => {
            let mut out = error.clone();
            if *truncated {
                out.push('\u{2026}');
                out.push('\n');
                out.push_str("(ctrl+o to expand)");
            }
            let unit = if *retry_in_seconds == 1 {
                "second"
            } else {
                "seconds"
            };
            out.push('\n');
            out.push_str(&format!(
                "Retrying in {retry_in_seconds} {unit}\u{2026} (attempt {retry_attempt}/{max_retries})"
            ));
            out
        }
        // error text + optional dim upsell line.
        RenderedMessage::RateLimit { text, upsell } => match upsell {
            Some(u) => format!("{text}\n{u}"),
            None => text.clone(),
        },
        // header + optional `Reason:` line + (rejected) tail line.
        RenderedMessage::Shutdown {
            from,
            reason,
            rejected,
        } => {
            let mut out = if *rejected {
                format!("Shutdown rejected by {from}")
            } else {
                format!("Shutdown request from {from}")
            };
            if let Some(r) = reason {
                out.push('\n');
                out.push_str(&format!("Reason: {r}"));
            }
            if *rejected {
                out.push('\n');
                out.push_str(
                    "Teammate is continuing to work. You may request shutdown again later.",
                );
            }
            out
        }
        // Measurement == render by construction: route through the renderer's
        // own string oracle. The verbose `Result` body counts the SAME
        // markdown-FLATTENED lines the component draws (raw `text` over-counts
        // dropped ``` fence rows / trailing blanks); the other kinds remain
        // their fixed one-liners.
        RenderedMessage::Advisor { kind, verbose } => {
            crate::components::messages::advisor::render_advisor_to_string(
                crate::components::messages::advisor::AdvisorProps {
                    kind: kind.clone(),
                    verbose: *verbose,
                },
            )
        }
        // Single dim line (running or transcript summary).
        RenderedMessage::HookProgress {
            event,
            count,
            transcript_summary,
        } => {
            if *transcript_summary {
                let unit = if *count == 1 { "hook" } else { "hooks" };
                format!("{count} {event} {unit} ran")
            } else {
                let unit = if *count == 1 {
                    "hook\u{2026}"
                } else {
                    "hooks\u{2026}"
                };
                format!("Running {event} {unit}")
            }
        }
        // Mirrors `render_plan_approval_to_string`.
        RenderedMessage::PlanApproval { kind } => match kind {
            crate::state::PlanApprovalKind::Request {
                from,
                plan_content,
                plan_file_path,
            } => {
                let mut out = format!("Plan Approval Request from {from}\n");
                out.push_str(plan_content);
                if let Some(p) = plan_file_path {
                    out.push('\n');
                    out.push_str(&format!("Plan file: {p}"));
                }
                out
            }
            crate::state::PlanApprovalKind::Approved { name } => {
                format!("\u{2713} Plan Approved by {name}\nYou can now proceed with implementation. Your plan mode restrictions have been lifted.")
            }
            crate::state::PlanApprovalKind::Rejected { name, feedback } => {
                let mut out = format!("\u{2717} Plan Rejected by {name}");
                if let Some(f) = feedback {
                    out.push('\n');
                    out.push_str(&format!("Feedback: {f}"));
                }
                out.push('\n');
                out.push_str(
                    "Please revise your plan based on the feedback and call ExitPlanMode again.",
                );
                out
            }
        },
    }
}

/// Per-message rendered-height cache. Maps message-index → line count at a
/// fixed viewport width. Backs all scroll-offset math. Rebuilt on width
/// change; appended to as new messages arrive (callers may simply rebuild
/// — `build` is O(n) over a 5k log and runs at most once per width change).
///
/// Alongside the per-message `heights`, the cache keeps a **prefix sum** of
/// cumulative line offsets (`prefix[i]` = absolute start line of message
/// `i`; `prefix[len] == total`). The prefix array makes two operations
/// O(1)/O(log n) instead of O(n):
/// - [`Self::span_start`] — the absolute start line of a message (O(1)).
/// - [`Self::message_index_at_line`] — the first message whose span
///   contains a given line, via binary search (O(log n)). This is what lets
///   [`render_window`] skip directly to the window start instead of walking
///   the whole log from index 0.
#[derive(Debug, Clone, Default)]
pub struct HeightCache {
    heights: Vec<usize>,
    /// Cumulative line offsets. `prefix.len() == heights.len() + 1`;
    /// `prefix[i]` is the absolute start line of message `i`, and
    /// `prefix[heights.len()] == total`. Empty caches keep `prefix == [0]`
    /// is NOT guaranteed — `Default` yields an empty `Vec`; treat an empty
    /// `prefix` as "no messages" (`span_start`/lookups fall back to 0/total).
    prefix: Vec<usize>,
    total: usize,
    width: usize,
}

impl HeightCache {
    /// Build the cache for `messages` at `width` columns.
    #[must_use]
    pub fn build(messages: &[RenderedMessage], width: usize) -> Self {
        let heights: Vec<usize> = messages.iter().map(|m| measured_height(m, width)).collect();
        // Prefix sum: prefix[i] = sum(heights[0..i]); prefix.len() == n + 1.
        let mut prefix = Vec::with_capacity(heights.len() + 1);
        let mut acc = 0usize;
        prefix.push(0);
        for &h in &heights {
            acc += h;
            prefix.push(acc);
        }
        Self {
            heights,
            prefix,
            total: acc,
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

    /// Absolute start line of the message at `index` (sum of all prior
    /// heights). O(1) via the prefix sum. Out-of-range indices clamp to
    /// `total_lines` (the line just past the end).
    #[must_use]
    pub fn span_start(&self, index: usize) -> usize {
        self.prefix.get(index).copied().unwrap_or(self.total)
    }

    /// First message index whose line span contains `line`, i.e. the
    /// message `i` with `span_start(i) <= line < span_start(i+1)`. O(log n)
    /// via binary search over the prefix sum.
    ///
    /// `line` is clamped to `[0, total_lines)`; for `line >= total_lines`
    /// (or an empty cache) this returns the last valid index (or 0 when
    /// empty). This is the entry point [`render_window`] uses to jump
    /// straight to the window start instead of scanning from index 0.
    #[must_use]
    pub fn message_index_at_line(&self, line: usize) -> usize {
        if self.heights.is_empty() {
            return 0;
        }
        // `prefix` is sorted ascending. `partition_point` finds the count of
        // entries `<= line`; the message starting at-or-before `line` is one
        // before that boundary. With prefix = [0, h0, h0+h1, …, total]:
        //   partition_point(p <= line) gives k where prefix[k-1] <= line.
        // The owning message index is k-1, clamped to the last message.
        let k = self.prefix.partition_point(|&p| p <= line);
        k.saturating_sub(1).min(self.heights.len() - 1)
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
///
/// ## Complexity — O(log n + window), NOT O(total)
/// The first visible message is located by binary search over the cache's
/// prefix sum ([`HeightCache::message_index_at_line`]); from there we walk
/// forward only while messages still intersect the viewport. The number of
/// messages touched is therefore proportional to the **window size**, not
/// the log length — at the default bottom-anchored `offset == 0` over a 5k
/// log we touch the handful of tail messages, never all 5000. The earlier
/// implementation looped from index 0 (O(depth-from-top), i.e. O(n) at
/// `offset == 0`); the
/// [`gate_window_walk_is_sublinear`](self) gate guards against regressing
/// to that linear walk.
#[must_use]
pub fn render_window(
    messages: &[RenderedMessage],
    cache: &HeightCache,
    scroll_offset: usize,
    viewport_height: usize,
) -> WindowSlice {
    render_window_counted(messages, cache, scroll_offset, viewport_height).0
}

/// Like [`render_window`] but also returns the **number of messages the
/// forward walk visited** after the binary-search jump. This is the
/// load-bearing perf metric: it must be ~window-sized, never O(total). The
/// public `render_window` delegates here and discards the count; the
/// [`gate_window_walk_is_sublinear`](self) gate calls this directly and
/// asserts the count stays sub-linear, so a regression to a from-index-0
/// linear walk (which would visit ~`last_index + 1` messages) trips it.
#[must_use]
pub fn render_window_counted(
    messages: &[RenderedMessage],
    cache: &HeightCache,
    scroll_offset: usize,
    viewport_height: usize,
) -> (WindowSlice, usize) {
    if messages.is_empty() || viewport_height == 0 || cache.total_lines() == 0 {
        return (
            WindowSlice {
                empty: true,
                ..WindowSlice::default()
            },
            0,
        );
    }
    let total = cache.total_lines();
    let max_offset = total.saturating_sub(viewport_height);
    let offset = scroll_offset.min(max_offset);

    // The visible line range is [top_line, bottom_line) in absolute lines
    // from the top of the log.
    let bottom_line = total - offset;
    let top_line = bottom_line.saturating_sub(viewport_height);

    // O(log n): jump straight to the first message whose span contains
    // `top_line` (the first one intersecting the viewport) — no scan from 0.
    let first_index = cache.message_index_at_line(top_line);
    let skip_top_lines = top_line.saturating_sub(cache.span_start(first_index));

    // O(window): walk forward from `first_index` only while messages keep
    // intersecting [top_line, bottom_line). A message intersects iff its
    // `span_start < bottom_line`; we stop at the first that doesn't. This
    // touches exactly the messages in the window, not the tail of the log.
    // `walked` counts every message this loop inspects (including the one
    // that triggers the break) so the gate can assert the work is bounded.
    let mut last_index = first_index;
    let mut walked = 0usize;
    let n = messages.len();
    for i in first_index..n {
        walked += 1;
        if cache.span_start(i) >= bottom_line {
            break;
        }
        last_index = i;
    }

    (
        WindowSlice {
            first_index,
            last_index,
            skip_top_lines,
            take_lines: viewport_height.min(total),
            empty: false,
        },
        walked,
    )
}

/// Props for [`VirtualMessageList`]. Mirrors the M6 `ScrollbackProps`
/// surface plus the line-based viewport. The full `messages` log is
/// passed; the component windows it.
///
/// ## Height cache is threaded in, NOT rebuilt
/// The per-frame render path passes the already-width-synced
/// [`HeightCache`] from `AppState` (kept fresh by `root.rs`'s
/// `refresh_height_cache(viewport_width)`, called immediately before each
/// render). The component does **not** call [`HeightCache::build`] — doing
/// so would be O(total messages) every frame and would defeat the whole
/// point of windowing. With the cache threaded in, the per-frame component
/// cost is O(window + log n).
///
/// `cache` MUST be consistent with `messages` (same length) and built for
/// `viewport_width`; `root.rs` guarantees this by calling
/// `refresh_height_cache` with the live viewport width before render.
#[derive(Default, Props)]
pub struct VirtualMessageListProps {
    /// Full retained message log (clone of `AppState::messages`).
    pub messages: Vec<RenderedMessage>,
    /// Pre-built, width-synced height cache (clone of
    /// `AppState::height_cache`). Backs the O(log n + window) windowing —
    /// the component never rebuilds it.
    pub cache: HeightCache,
    /// Line-based scroll offset (0 = latest at bottom).
    pub scroll_offset: usize,
    /// Viewport height in lines.
    pub viewport_height: usize,
    /// Per-tool expanded flags (clone of `AppState::expanded`).
    pub expanded: HashMap<ToolUseId, bool>,
    /// Focused tool id (clone of `AppState::focused_tool_id`).
    pub focused_tool_id: Option<ToolUseId>,
}

/// Windowed scrollback component. Renders only the messages whose line
/// spans intersect the viewport (+ overscan), not the whole log.
///
/// Per-frame cost is O(window + log n): the height cache is threaded in
/// (already built once per width change, never rebuilt here), and
/// [`render_window`] locates the window start via binary search.
#[component]
pub fn VirtualMessageList(props: &VirtualMessageListProps) -> impl Into<AnyElement<'static>> {
    // Overscan: render a few extra lines of viewport so a line-step does
    // not flash blank rows. Purely a visual buffer — offset math is exact.
    let vh = props.viewport_height.saturating_add(OVERSCAN_LINES);
    let win = render_window(&props.messages, &props.cache, props.scroll_offset, vh);

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

    // ---- (M7-03 review) Per-variant measurement lock tests --------------
    //
    // These pin `measured_height` for EVERY current `RenderedMessage`
    // variant so a careless edit to `render_text_for_measure` (the proxy)
    // — or to a `render_message` renderer that the proxy is supposed to
    // mirror — trips a test. Expected counts are hand-specified (the real
    // `render_message` returns an iocraft element, not text, so we cannot
    // derive them from the render path here) and annotated with WHY they
    // hold. When you add a variant or change a renderer's line layout, you
    // MUST update `render_text_for_measure` and these pins together.
    //
    // KNOWN PROXY LIMITATION pinned on purpose: for `UserToolResult` the
    // proxy measures only the `result` payload — it does NOT account for the
    // collapsed first-line/`(+N lines)` form, line/byte truncation, or the
    // M7-02 diff body (`old_string`/`new_string`/`file_path`). The diff case
    // below documents that current behavior so the eventual M7-04/05
    // measurement↔renderer unification deliberately changes these pins.

    #[test]
    fn measured_height_pins_user_text() {
        // UserTextMessage draws the body verbatim (the `> ` prefix adds
        // columns, not rows). 3 newline-separated lines → 3 rows.
        let m = RenderedMessage::UserText {
            body: "l1\nl2\nl3".into(),
            timestamp: 0,
        };
        assert_eq!(measured_height(&m, 80), 3);
    }

    #[test]
    fn measured_height_pins_assistant_text() {
        // AssistantTextMessage draws the body verbatim (`● ` prefix = cols).
        let m = RenderedMessage::AssistantText {
            body: "one\ntwo".into(),
            timestamp: 0,
        };
        assert_eq!(measured_height(&m, 80), 2);
    }

    #[test]
    fn measured_height_pins_system_text() {
        // SystemText draws the body verbatim (color only, no extra rows).
        let m = RenderedMessage::SystemText {
            body: "a\nb\nc\nd".into(),
            timestamp: 0,
            is_error: false,
        };
        assert_eq!(measured_height(&m, 80), 4);
    }

    #[test]
    fn measured_height_pins_assistant_tool_use() {
        // Proxy renders the single-line collapsed form `● {tool}(…)`.
        // Width 80 → 1 row regardless of `input`.
        let m = RenderedMessage::AssistantToolUse {
            id: ToolUseId::new(),
            tool: "Read".into(),
            input: serde_json::json!({ "file_path": "/x", "extra": [1, 2, 3] }),
        };
        assert_eq!(measured_height(&m, 80), 1);
    }

    #[test]
    fn measured_height_pins_user_tool_result_plain_string() {
        // Proxy uses the result string body. 2 lines → 2 rows.
        let m = RenderedMessage::UserToolResult {
            id: ToolUseId::new(),
            tool: "Bash".into(),
            result: serde_json::json!("line1\nline2"),
            old_string: None,
            new_string: None,
            file_path: None,
        };
        assert_eq!(measured_height(&m, 80), 2);
    }

    #[test]
    fn measured_height_pins_user_tool_result_edit_diff() {
        // Edit-diff case: old/new strings + file_path are set, but the
        // CURRENT proxy measures only the `result` payload (it ignores the
        // diff fields). A non-string `result` falls back to `to_string()`
        // → the JSON literal `"applied"` (with quotes), one line → 1 row.
        // This pins the known proxy/renderer drift so the M7-04/05
        // unification must consciously revise it.
        let m = RenderedMessage::UserToolResult {
            id: ToolUseId::new(),
            tool: "Edit".into(),
            result: serde_json::json!("applied"),
            old_string: Some("fn a() {}\nold line".into()),
            new_string: Some("fn a() {}\nnew line\nextra".into()),
            file_path: Some("/src/a.rs".into()),
        };
        // Proxy text == "applied" (str body) → 1 row. The 2-line old / 3-line
        // new diff body is NOT counted by today's proxy — documented above.
        assert_eq!(measured_height(&m, 80), 1);
    }

    // ---- (M7-04) Per-variant measurement lock tests --------------------
    //
    // Pin `measured_height` for each of the 10 batch-1 variants so the proxy
    // stays in lock-step with the `render_*_to_string` renderers. When a
    // renderer changes how a variant lays out rows you MUST update both the
    // proxy arm in `render_text_for_measure` AND the pin here.

    #[test]
    fn measured_height_pins_assistant_thinking_m7_04() {
        // Collapsed → single header+hint line.
        let collapsed = RenderedMessage::AssistantThinking {
            thinking: "anything".into(),
            expanded: false,
        };
        assert_eq!(measured_height(&collapsed, 80), 1);
        // Expanded → header `∴ Thinking…` (1) + 2 body lines (indented 2) = 3.
        let expanded = RenderedMessage::AssistantThinking {
            thinking: "Step one.\nStep two.".into(),
            expanded: true,
        };
        assert_eq!(measured_height(&expanded, 80), 3);
    }

    // ---- (M7-04 review) measurement == render for markdown bodies --------
    //
    // The expanded thinking / verbose advisor renderers flatten their body
    // through `render::markdown` (fenced code blocks drop the ``` fences,
    // trailing blank lines collapse). `measured_height` MUST count the SAME
    // flattened text the renderer draws, so we derive the expected row count
    // from the renderer's own string output rather than the raw input. These
    // tests FAIL against a raw-line proxy (which over-counts the dropped
    // fence/blank rows) and pass once measurement routes through the
    // renderer's flattening.

    #[test]
    fn measured_height_thinking_expanded_matches_renderer_fenced_code() {
        use crate::components::messages::thinking::{render_thinking_to_string, ThinkingProps};
        // Fenced code block: the ``` fence lines are dropped by markdown
        // flattening, so the renderer draws fewer rows than the raw input.
        let body = "intro\n```rust\nlet x = 1;\n```\noutro";
        let msg = RenderedMessage::AssistantThinking {
            thinking: body.into(),
            expanded: true,
        };
        let rendered = render_thinking_to_string(ThinkingProps {
            thinking: body.into(),
            expanded: true,
        });
        let expected_rows = rendered.lines().count();
        assert_eq!(
            measured_height(&msg, 80),
            expected_rows,
            "measured_height must equal the renderer's flattened row count"
        );
    }

    #[test]
    fn measured_height_thinking_expanded_matches_renderer_trailing_blank() {
        use crate::components::messages::thinking::{render_thinking_to_string, ThinkingProps};
        // Trailing blank line: markdown flattening drops it, so the renderer
        // draws one fewer row than the raw `.lines()` proxy would (raw
        // `"text\n\n".lines()` yields `["text", ""]` → 2 body rows, but the
        // flattened body is just `"text"` → 1 body row).
        let body = "text\n\n";
        let msg = RenderedMessage::AssistantThinking {
            thinking: body.into(),
            expanded: true,
        };
        let rendered = render_thinking_to_string(ThinkingProps {
            thinking: body.into(),
            expanded: true,
        });
        assert_eq!(measured_height(&msg, 80), rendered.lines().count());
    }

    #[test]
    fn measured_height_pins_redacted_thinking_m7_04() {
        // `✻ Thinking…` — single line.
        assert_eq!(
            measured_height(&RenderedMessage::AssistantRedactedThinking, 80),
            1
        );
    }

    #[test]
    fn measured_height_pins_compact_boundary_m7_04() {
        // Single boundary line; counts are not rendered.
        let m = RenderedMessage::CompactBoundary {
            messages_before: 50,
            messages_after: 5,
        };
        assert_eq!(measured_height(&m, 80), 1);
    }

    #[test]
    fn measured_height_pins_system_text_rich_m7_04() {
        // Info → body verbatim (3 lines). Warning → `● ` marker (cols) + body
        // (1 line). Both: marker adds columns, not rows.
        let info = RenderedMessage::SystemTextRich {
            body: "a\nb\nc".into(),
            level: crate::state::SystemLevel::Info,
        };
        assert_eq!(measured_height(&info, 80), 3);
        let warn = RenderedMessage::SystemTextRich {
            body: "Approaching context limit.".into(),
            level: crate::state::SystemLevel::Warning,
        };
        assert_eq!(measured_height(&warn, 80), 1);
    }

    #[test]
    fn measured_height_pins_system_api_error_m7_04() {
        // Not truncated → error body (1) + retry footer (1) = 2 lines.
        let plain = RenderedMessage::SystemApiError {
            error: "529 Overloaded".into(),
            retry_attempt: 4,
            retry_in_seconds: 3,
            max_retries: 10,
            truncated: false,
        };
        assert_eq!(measured_height(&plain, 80), 2);
        // Truncated → error+`…` (1) + expand-hint (1) + retry footer (1) = 3.
        let trunc = RenderedMessage::SystemApiError {
            error: "boom".into(),
            retry_attempt: 5,
            retry_in_seconds: 1,
            max_retries: 10,
            truncated: true,
        };
        assert_eq!(measured_height(&trunc, 80), 3);
    }

    #[test]
    fn measured_height_pins_rate_limit_m7_04() {
        // No upsell → 1 line. With upsell → 2 lines.
        let no_upsell = RenderedMessage::RateLimit {
            text: "You've hit your usage limit.".into(),
            upsell: None,
        };
        assert_eq!(measured_height(&no_upsell, 80), 1);
        let with_upsell = RenderedMessage::RateLimit {
            text: "You've hit your usage limit.".into(),
            upsell: Some("/upgrade to increase your usage limit.".into()),
        };
        assert_eq!(measured_height(&with_upsell, 80), 2);
    }

    #[test]
    fn measured_height_pins_shutdown_m7_04() {
        // Request + reason → header (1) + Reason (1) = 2 lines.
        let request = RenderedMessage::Shutdown {
            from: "agent-2".into(),
            reason: Some("task done".into()),
            rejected: false,
        };
        assert_eq!(measured_height(&request, 80), 2);
        // Rejected + reason → header (1) + Reason (1) + tail (1) = 3 lines.
        let rejected = RenderedMessage::Shutdown {
            from: "agent-2".into(),
            reason: Some("still working".into()),
            rejected: true,
        };
        assert_eq!(measured_height(&rejected, 80), 3);
    }

    #[test]
    fn measured_height_pins_advisor_m7_04() {
        // Non-verbose result → single review line.
        let result = RenderedMessage::Advisor {
            kind: crate::state::AdvisorKind::Result {
                text: "Looks good.".into(),
            },
            verbose: false,
        };
        assert_eq!(measured_height(&result, 80), 1);
        // Error → single `Advisor unavailable (…)` line.
        let err = RenderedMessage::Advisor {
            kind: crate::state::AdvisorKind::Error {
                error_code: "503".into(),
            },
            verbose: false,
        };
        assert_eq!(measured_height(&err, 80), 1);
    }

    #[test]
    fn measured_height_advisor_verbose_matches_renderer_fenced_code() {
        use crate::components::messages::advisor::{render_advisor_to_string, AdvisorProps};
        // Verbose result body flows through markdown flattening, so the ```
        // fence lines are dropped — measurement must match the renderer's
        // flattened row count, not the raw `.lines()` proxy.
        let text = "summary\n```\ncode line\n```\ntail";
        let kind = crate::state::AdvisorKind::Result { text: text.into() };
        let msg = RenderedMessage::Advisor {
            kind: kind.clone(),
            verbose: true,
        };
        let rendered = render_advisor_to_string(AdvisorProps {
            kind,
            verbose: true,
        });
        assert_eq!(
            measured_height(&msg, 80),
            rendered.lines().count(),
            "verbose advisor measurement must equal the renderer's flattened row count"
        );
    }

    #[test]
    fn measured_height_pins_hook_progress_m7_04() {
        // Running plural → single line.
        let running = RenderedMessage::HookProgress {
            event: "SessionStart".into(),
            count: 3,
            transcript_summary: false,
        };
        assert_eq!(measured_height(&running, 80), 1);
        // Transcript singular → single line.
        let transcript = RenderedMessage::HookProgress {
            event: "PreToolUse".into(),
            count: 1,
            transcript_summary: true,
        };
        assert_eq!(measured_height(&transcript, 80), 1);
    }

    #[test]
    fn measured_height_pins_plan_approval_m7_04() {
        // Request → header (1) + 2 plan-content lines (1+1) + Plan file (1) = 4.
        let request = RenderedMessage::PlanApproval {
            kind: crate::state::PlanApprovalKind::Request {
                from: "agent-3".into(),
                plan_content: "1. Do X\n2. Do Y".into(),
                plan_file_path: Some("/tmp/plan.md".into()),
            },
        };
        assert_eq!(measured_height(&request, 80), 4);
        // Approved → header (1) + tail. The tail line
        // "You can now proceed with implementation. Your plan mode
        // restrictions have been lifted." is 86 cols → wraps to 2 rows at
        // width 80, so total = 3 rows. (At a wider width it would be 2.)
        let approved = RenderedMessage::PlanApproval {
            kind: crate::state::PlanApprovalKind::Approved { name: "you".into() },
        };
        assert_eq!(measured_height(&approved, 80), 3);
        // Sanity: at a width that holds the tail on one line, total = 2 rows.
        assert_eq!(measured_height(&approved, 120), 2);
        // Rejected + feedback → header (1) + Feedback (1) + tail (1) = 3 lines.
        let rejected = RenderedMessage::PlanApproval {
            kind: crate::state::PlanApprovalKind::Rejected {
                name: "you".into(),
                feedback: Some("too risky".into()),
            },
        };
        assert_eq!(measured_height(&rejected, 80), 3);
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
    fn span_start_is_cumulative_prefix_sum() {
        // heights [1, 3, 1] → starts [0, 1, 4]; span_start(len) == total.
        let msgs = vec![user("one"), user("a\nb\nc"), user("two")];
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.span_start(0), 0);
        assert_eq!(cache.span_start(1), 1);
        assert_eq!(cache.span_start(2), 4);
        assert_eq!(cache.span_start(3), 5); // one past the end == total
        assert_eq!(cache.span_start(99), 5); // out of range clamps to total
    }

    #[test]
    fn message_index_at_line_binary_search() {
        // heights [1, 3, 1] → spans m0=[0,1) m1=[1,4) m2=[4,5).
        let msgs = vec![user("one"), user("a\nb\nc"), user("two")];
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.message_index_at_line(0), 0); // start of m0
        assert_eq!(cache.message_index_at_line(1), 1); // start of m1
        assert_eq!(cache.message_index_at_line(2), 1); // inside m1
        assert_eq!(cache.message_index_at_line(3), 1); // last line of m1
        assert_eq!(cache.message_index_at_line(4), 2); // start of m2
                                                       // line >= total clamps to the last valid index.
        assert_eq!(cache.message_index_at_line(5), 2);
        assert_eq!(cache.message_index_at_line(999), 2);
    }

    #[test]
    fn message_index_at_line_empty_cache_is_zero() {
        let cache = HeightCache::build(&[], 80);
        assert_eq!(cache.message_index_at_line(0), 0);
        assert_eq!(cache.message_index_at_line(42), 0);
    }

    #[test]
    fn message_index_at_line_matches_linear_scan_over_mixed_log() {
        // Cross-check the O(log n) binary search against a brute-force
        // linear scan for every line of a mixed-height log.
        let msgs = mixed_log(); // heights [1, 50, 1, 1], total 53
        let cache = HeightCache::build(&msgs, 80);
        let total = cache.total_lines();
        for line in 0..total {
            // Linear reference: first message whose span_end > line.
            let mut acc = 0usize;
            let mut expected = 0usize;
            for i in 0..msgs.len() {
                let end = acc + cache.height_at(i);
                if end > line {
                    expected = i;
                    break;
                }
                acc = end;
            }
            assert_eq!(
                cache.message_index_at_line(line),
                expected,
                "mismatch at line {line}"
            );
        }
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
