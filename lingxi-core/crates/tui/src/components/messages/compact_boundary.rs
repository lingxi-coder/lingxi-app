//! `CompactBoundaryMessage` — `✻ Conversation compacted (ctrl+o for history)`.
//!
//! Literal lock (claude-code `CompactBoundaryMessage.tsx`): `dimColor`,
//! `marginY 1`. Shortcut literal `ctrl+o`. REPLACES M6-08's
//! `[Compacted N → M messages]` `SystemText` placeholder (see streaming.rs
//! `CompactionCompleted` handler).
use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Locked boundary line. `✻` = U+273B.
pub const BOUNDARY_LINE: &str = "\u{273B} Conversation compacted (ctrl+o for history)";

/// Pure-string renderer (the line is fixed; counts are not rendered — parity).
#[must_use]
pub fn render_compact_boundary_to_string() -> String {
    BOUNDARY_LINE.to_string()
}

/// iocraft component — dim.
#[component]
pub fn CompactBoundaryMessage() -> impl Into<AnyElement<'static>> {
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: BOUNDARY_LINE, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundary_line_starts_with_teardrop() {
        // ✻ = U+273B = 0xE2 0x9C 0xBB.
        assert_eq!(&BOUNDARY_LINE.as_bytes()[..3], &[0xE2, 0x9C, 0xBB]);
    }
}
