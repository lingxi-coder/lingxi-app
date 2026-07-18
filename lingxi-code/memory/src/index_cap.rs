//! MEMORY.md index over-cap advisory (claude-code 2.1.210+).
//!
//! When a Write/Edit leaves the memory index (`MEMORY.md`) over — or
//! approaching — its read limit, claude-code surfaces an explicit advisory so
//! the model rewrites the index instead of silently losing entries past the cap
//! each time it is loaded. This module is the byte-faithful port of that
//! advisory generator.
//!
//! ## Ported functions (2.1.212 binary)
//!
//! - `qAt(content)` → [`measure`]: trims the content, then reports
//!   `lineCount = <count of '\n' in trimmed> + 1` and `byteCount = trimmed len`.
//! - `ypo({rawSizeBytes, surfaceCap, splicedSizeBytes, spliceCap, spliceActive})`
//!   → resolves the `(sizeBytes, byteCap)` pair. For the plain memdir entrypoint
//!   `surfaceCap` (`promptIndexMaxBytes`) is undefined, so `ypo` reduces to
//!   `{sizeBytes: splicedSizeBytes, byteCap: spliceCap}` — i.e. the loaded byte
//!   count measured against `GCe = 25000` ([`crate::MAX_ENTRYPOINT_BYTES`]).
//! - `bpo({label, displayPath, sizeBytes, byteCap, lineCount, lineCap})`
//!   → [`memory_index_cap_notice`]: takes the max-`frac` dimension across the
//!   byte and line caps and, when it is at/over the `TQg = 0.8` surface
//!   threshold, renders the over-limit (`Error: this write left …`) or the
//!   approaching (`… approaching the … read limit. Compact it`) advisory,
//!   pointing at `JWu = 0.7` × cap as the rewrite target.
//! - `Ua(bytes)` → [`human_bytes`]: the `bytes`/`KB`/`MB`/`GB` size formatter.
//!
//! The write-hook that actually delivers the advisory (`IZg`/`XGu`, a
//! `PostToolUse` callback on Write/Edit that emits
//! [`TENGU_MEMDIR_ENTRYPOINT_NEAR_CAP`] with an `over_cap` flag and returns
//! [`MemoryIndexNotice::text`] as `additionalContext`) is a composition-root
//! activation step: the memdir-entrypoint `PostToolUse` surface is not yet wired
//! in the engine. This generator is the reusable core it calls — pure,
//! byte-exact, and driven by the (previously dead) entrypoint caps
//! [`crate::MAX_ENTRYPOINT_BYTES`] / [`crate::MAX_ENTRYPOINT_LINES`].
#![forbid(unsafe_code)]

use crate::{MAX_ENTRYPOINT_BYTES, MAX_ENTRYPOINT_LINES};

/// Telemetry event the write-hook emits when the memory index is at/over cap,
/// carrying an `over_cap: bool` field (claude-code `XGu` →
/// `M("tengu_memdir_entrypoint_near_cap", { over_cap, … })`).
pub const TENGU_MEMDIR_ENTRYPOINT_NEAR_CAP: &str = "tengu_memdir_entrypoint_near_cap";

/// Display name of the memory index in the advisory (claude-code `o0`).
const MEMORY_INDEX_DISPLAY_PATH: &str = "MEMORY.md";

// Surface threshold (claude-code `TQg = 0.8`) and rewrite target (`JWu = 0.7`)
// are applied with exact integer arithmetic to match the crate's fixed-point
// convention and avoid `usize`→`f64` rounding:
//   `frac >= 0.8`            ⟺ `count * 5 >= cap * 4`
//   `Math.floor(cap * 0.7)`  ⟺ `cap * 7 / 10`
// (`count * 10 >= cap * 8` reduced by 2; `count / cap > other / otherCap`
// cross-multiplies to `count * otherCap > other * cap`.)

/// The rendered advisory plus whether the index is strictly over cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryIndexNotice {
    /// Model-facing advisory text (delivered as a `PostToolUse`
    /// `additionalContext`).
    pub text: String,
    /// `true` when the worst dimension is strictly over its cap (the `Error:`
    /// wording); `false` for the "approaching … Compact it" wording. Mirrors
    /// `bpo(...).overCap`, the telemetry `over_cap` field.
    pub over_cap: bool,
}

/// Trimmed line/byte measurement of memory-index content (claude-code `qAt`).
///
/// `line_count = <'\n' occurrences in the trimmed content> + 1`;
/// `byte_count = <trimmed UTF-8 length>` (claude-code measures `String.length`
/// UTF-16 units — identical for the ASCII index lines the format prescribes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexMeasure {
    /// Line count of the trimmed content (always ≥ 1).
    pub line_count: usize,
    /// Byte count of the trimmed content.
    pub byte_count: usize,
}

/// Measure `content` the way claude-code `qAt` does: trim, then count.
#[must_use]
pub fn measure(content: &str) -> IndexMeasure {
    let trimmed = content.trim();
    IndexMeasure {
        line_count: trimmed.matches('\n').count() + 1,
        byte_count: trimmed.len(),
    }
}

/// Format a byte size as claude-code `Ua` does: `<n> bytes` under 1 KiB, then
/// `KB`/`MB`/`GB` at one decimal with a trailing `.0` trimmed.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "Ua formats human-facing sizes; memory-index bytes are far below the f64 mantissa limit"
)]
pub fn human_bytes(bytes: usize) -> String {
    let t = bytes as f64 / 1024.0;
    if t < 1.0 {
        return format!("{bytes} bytes");
    }
    if t < 1024.0 {
        return format!("{}KB", one_decimal(t));
    }
    let r = t / 1024.0;
    if r < 1024.0 {
        return format!("{}MB", one_decimal(r));
    }
    format!("{}GB", one_decimal(r / 1024.0))
}

/// `x.toFixed(1)` with a trailing `.0` stripped (claude-code
/// `.toFixed(1).replace(/\.0$/,"")`).
fn one_decimal(x: f64) -> String {
    let s = format!("{x:.1}");
    match s.strip_suffix(".0") {
        Some(head) => head.to_string(),
        None => s,
    }
}

/// One cap dimension considered by [`memory_index_cap_notice`] (claude-code
/// `bpo`'s `t[]` entries): a `size/cap` pair plus its rendered descriptions.
struct Dimension {
    size: usize,
    cap: usize,
    over: bool,
    size_desc: String,
    cap_desc: String,
    target_desc: String,
}

impl Dimension {
    /// `true` when this dimension's fill fraction strictly exceeds `other`'s
    /// (`self.size / self.cap > other.size / other.cap`, cross-multiplied).
    fn fraction_exceeds(&self, other: &Self) -> bool {
        self.size * other.cap > other.size * self.cap
    }

    /// `true` when this dimension is at/over the 0.8 surface threshold
    /// (`size / cap >= 0.8` ⟺ `size * 5 >= cap * 4`).
    fn at_surface_threshold(&self) -> bool {
        self.size * 5 >= self.cap * 4
    }
}

/// Build the memory-index over/near-cap advisory for `content`, or `None` when
/// the index is comfortably under both caps.
///
/// Byte-faithful port of claude-code `bpo` (with `ypo` reduced to the plain
/// memdir-entrypoint case): the byte dimension is measured against
/// [`crate::MAX_ENTRYPOINT_BYTES`] and the line dimension against
/// [`crate::MAX_ENTRYPOINT_LINES`]; the dimension with the larger fill fraction
/// wins (ties keep the byte dimension, matching the strict-`>` reduce). When the
/// winner is below 0.8 of its cap the result is `None`.
#[must_use]
pub fn memory_index_cap_notice(content: &str) -> Option<MemoryIndexNotice> {
    let m = measure(content);
    memory_index_cap_notice_measured(m.byte_count, m.line_count)
}

/// [`memory_index_cap_notice`] over an already-measured `(byte_count,
/// line_count)` pair (claude-code `bpo` proper — the measurement is `qAt`).
#[must_use]
pub fn memory_index_cap_notice_measured(
    byte_count: usize,
    line_count: usize,
) -> Option<MemoryIndexNotice> {
    // Byte dimension is first, so a tie in fraction keeps it (reduce uses `>`).
    let byte_dim = Dimension {
        size: byte_count,
        cap: MAX_ENTRYPOINT_BYTES,
        over: byte_count > MAX_ENTRYPOINT_BYTES,
        size_desc: human_bytes(byte_count),
        cap_desc: human_bytes(MAX_ENTRYPOINT_BYTES),
        target_desc: human_bytes(MAX_ENTRYPOINT_BYTES * 7 / 10),
    };
    let line_dim = Dimension {
        size: line_count,
        cap: MAX_ENTRYPOINT_LINES,
        over: line_count > MAX_ENTRYPOINT_LINES,
        size_desc: format!("{line_count} lines"),
        cap_desc: format!("{MAX_ENTRYPOINT_LINES}-line"),
        target_desc: format!("{} lines", MAX_ENTRYPOINT_LINES * 7 / 10),
    };

    let worst = if line_dim.fraction_exceeds(&byte_dim) {
        &line_dim
    } else {
        &byte_dim
    };

    if !worst.at_surface_threshold() {
        return None;
    }

    let text = if worst.over {
        format!(
            "Error: this write left the memory index at {path} at {size}, over its {cap} \
             read limit. The write succeeded, but everything past the limit is silently \
             dropped each time the index is loaded \u{2014} entries at the end are already \
             invisible to readers. Rewrite it to under {target} now: keep one line per \
             entry, move detail into topic files, and merge or drop stale entries.",
            path = MEMORY_INDEX_DISPLAY_PATH,
            size = worst.size_desc,
            cap = worst.cap_desc,
            target = worst.target_desc,
        )
    } else {
        format!(
            "The memory index at {path} is {size}, approaching the {cap} read limit. \
             Compact it to under {target} now: keep one line per entry, move detail into \
             topic files, and merge or drop stale entries.",
            path = MEMORY_INDEX_DISPLAY_PATH,
            size = worst.size_desc,
            cap = worst.cap_desc,
            target = worst.target_desc,
        )
    };

    Some(MemoryIndexNotice {
        text,
        over_cap: worst.over,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_both_caps_is_none() {
        // A tiny index — well under 0.8 of every cap.
        assert!(memory_index_cap_notice("- [A](a.md) \u{2014} hook\n").is_none());
        // Exactly at the byte threshold boundary just below 0.8: 19_999 bytes.
        assert!(memory_index_cap_notice_measured(19_999, 10).is_none());
    }

    #[test]
    fn human_bytes_matches_ua() {
        assert_eq!(human_bytes(500), "500 bytes");
        assert_eq!(human_bytes(1024), "1KB");
        assert_eq!(human_bytes(MAX_ENTRYPOINT_BYTES), "24.4KB");
        // 0.7 * 25000 = 17500 bytes -> 17.1KB.
        assert_eq!(human_bytes(17_500), "17.1KB");
        assert_eq!(human_bytes(1024 * 1024), "1MB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1GB");
    }

    #[test]
    fn approaching_byte_cap_renders_compact_wording() {
        // 0.8 <= frac < 1.0 on bytes, lines comfortably under -> "approaching".
        let notice = memory_index_cap_notice_measured(20_000, 20).expect("should surface");
        assert!(!notice.over_cap);
        assert_eq!(
            notice.text,
            "The memory index at MEMORY.md is 19.5KB, approaching the 24.4KB read limit. \
             Compact it to under 17.1KB now: keep one line per entry, move detail into \
             topic files, and merge or drop stale entries."
        );
    }

    #[test]
    fn over_byte_cap_renders_error_wording() {
        let notice = memory_index_cap_notice_measured(30_000, 20).expect("should surface");
        assert!(notice.over_cap);
        assert_eq!(
            notice.text,
            "Error: this write left the memory index at MEMORY.md at 29.3KB, over its 24.4KB \
             read limit. The write succeeded, but everything past the limit is silently \
             dropped each time the index is loaded \u{2014} entries at the end are already \
             invisible to readers. Rewrite it to under 17.1KB now: keep one line per entry, \
             move detail into topic files, and merge or drop stale entries."
        );
    }

    #[test]
    fn over_line_cap_wins_when_lines_dominate() {
        // Small bytes, but 210 lines -> line dimension is the worst and over cap.
        let notice = memory_index_cap_notice_measured(1_000, 210).expect("should surface");
        assert!(notice.over_cap);
        assert!(
            notice.text.starts_with(
                "Error: this write left the memory index at MEMORY.md at 210 lines, over its \
                 200-line read limit."
            ),
            "got: {}",
            notice.text
        );
        assert!(notice.text.contains("Rewrite it to under 140 lines now:"));
    }

    #[test]
    fn near_line_cap_renders_line_compact_wording() {
        // 170 lines -> frac 0.85 on lines (>= 0.8), not over.
        let notice = memory_index_cap_notice_measured(1_000, 170).expect("should surface");
        assert!(!notice.over_cap);
        assert!(
            notice.text.starts_with(
                "The memory index at MEMORY.md is 170 lines, approaching the 200-line read limit."
            ),
            "got: {}",
            notice.text
        );
        assert!(notice.text.contains("Compact it to under 140 lines now:"));
    }

    #[test]
    fn measure_trims_then_counts_like_qat() {
        // Leading/trailing whitespace is trimmed before measuring (qAt).
        let m = measure("\n\n  a\nb\nc  \n\n");
        assert_eq!(m.line_count, 3); // "a\nb\nc" -> 2 newlines + 1
        assert_eq!(m.byte_count, 5); // "a\nb\nc"
    }

    #[test]
    fn measure_backed_notice_matches_direct() {
        // Build a >0.8-byte index by content and confirm the string path agrees
        // with the measured path.
        let body = "x".repeat(21_000);
        let via_content = memory_index_cap_notice(&body).unwrap();
        let via_measure = memory_index_cap_notice_measured(21_000, 1).unwrap();
        assert_eq!(via_content, via_measure);
    }

    #[test]
    fn event_name_is_byte_faithful() {
        assert_eq!(
            TENGU_MEMDIR_ENTRYPOINT_NEAR_CAP,
            "tengu_memdir_entrypoint_near_cap"
        );
    }
}
