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

/// Pure-string renderer: a (compact-boundary-marginy) blank line above and
/// below the boundary line (claude-code `marginY={1}`; counts are not
/// rendered — parity).
#[must_use]
pub fn render_compact_boundary_to_string() -> String {
    format!("\n{BOUNDARY_LINE}\n")
}

/// iocraft component — dim, with a blank row above and below (marginY={1}
/// has no single-View equivalent in iocraft's flex model, so this renders
/// three explicit rows rather than embedding leading/trailing newlines in
/// one `Text` — iocraft doesn't lay that out the same way, see plan.rs).
#[component]
pub fn CompactBoundaryMessage() -> impl Into<AnyElement<'static>> {
    element! {
        View(flex_direction: FlexDirection::Column) {
            View(height: 1)
            Text(content: BOUNDARY_LINE, color: TuiTheme::DIM)
            View(height: 1)
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

    #[test]
    fn blank_line_above_and_below() {
        // (compact-boundary-marginy)
        assert_eq!(
            render_compact_boundary_to_string(),
            format!("\n{BOUNDARY_LINE}\n")
        );
    }
}
