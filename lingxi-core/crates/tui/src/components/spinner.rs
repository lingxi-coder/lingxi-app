#![forbid(unsafe_code)]
//! `SpinnerWithVerb` — claude-code-equivalent loading spinner. (M6-03)
//!
//! Per `claude-code/src/components/Spinner/utils.ts` (darwin default) +
//! `Spinner.tsx:41` (`SPINNER_FRAMES = [...DEFAULT, ...reverse(DEFAULT)]`):
//! 6 forward chars then 6 reverse chars = 12 total.
//!
//! Linux platforms substitute `✳` → `*` (one frame difference); locked here
//! as the darwin variant since macOS is our primary dev platform and the
//! spec §2.8 requires byte-for-byte parity with claude-code. (A
//! `cfg(target_os = "linux")` variant can be added in M7 if needed.)

use iocraft::prelude::*;

/// 12-frame asterisk animation (forward+reverse cycle). claude-code's
/// darwin default characters from `Spinner/utils.ts`.
pub const SPINNER_FRAMES: &[&str] = &[
    "·", "✢", "✳", "✶", "✻", "✽", "✽", "✻", "✶", "✳", "✢", "·",
];

/// The 3-verb subset M6 uses (deterministically cycled every 4s for
/// testability). claude-code's full pool of 100+ verbs (see
/// `claude-code/src/constants/spinnerVerbs.ts`) is deferred to M7 along
/// with the random-on-mount selection logic.
pub const VERBS_M6: &[&str] = &["Crunching", "Thinking", "Generating"];

/// Time between spinner frame advances. claude-code uses 50ms (20fps);
/// M6 uses 100ms (10fps) per spec §3 M6-03 — slower to reduce render
/// churn under the 30fps cap.
pub const FRAME_TICK_MS: u64 = 100;

/// Time between verb rotations. Locked at 4000ms so all 3 verbs cycle
/// in 12s — long enough that users notice the change, short enough that
/// it doesn't feel static.
pub const VERB_ROTATE_MS: u64 = 4000;

/// Get the spinner glyph for a tick index. Wraps modulo `SPINNER_FRAMES.len()`.
#[inline]
#[must_use]
pub fn frame_at_index(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

/// Get the verb for a rotation index. Wraps modulo `VERBS_M6.len()`.
#[inline]
#[must_use]
pub fn verb_at_index(rotation: usize) -> &'static str {
    VERBS_M6[rotation % VERBS_M6.len()]
}

/// Format a single spinner line: `"{frame} {verb}…"`.
///
/// This is the function the iocraft component renders inside its `Text`.
/// Exposed publicly so snapshot tests can assert without a render harness.
/// The ellipsis is U+2026 (HORIZONTAL ELLIPSIS), NOT three ASCII dots.
#[must_use]
pub fn format_spinner_line(tick: usize, rotation: usize) -> String {
    format!("{} {}…", frame_at_index(tick), verb_at_index(rotation))
}

/// Props for [`SpinnerWithVerb`]. Both fields are hook-managed inside the
/// component by default; pass `Some(_)` to override (used by snapshot tests).
#[derive(Default, Props)]
pub struct SpinnerWithVerbProps {
    /// Override the frame index. `None` (default) → component ticks
    /// internally at `FRAME_TICK_MS`.
    pub frame_override: Option<usize>,
    /// Override the verb rotation index. `None` → internal rotation at
    /// `VERB_ROTATE_MS`.
    pub verb_override: Option<usize>,
}

/// Renders one line: `"{frame} {verb}…"`. While mounted, advances frames
/// at 10fps and rotates verbs every 4s. Both intervals are constants
/// (`FRAME_TICK_MS`, `VERB_ROTATE_MS`).
///
/// Mounting/unmounting is the caller's responsibility: the REPL screen
/// wraps this in `if app.streaming.is_some() { <SpinnerWithVerb/> }`.
#[component]
pub fn SpinnerWithVerb(
    props: &SpinnerWithVerbProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut frame = hooks.use_state(|| 0usize);
    let mut verb = hooks.use_state(|| 0usize);

    if let Some(f) = props.frame_override {
        frame.set(f);
    } else {
        hooks.use_future(async move {
            let mut tick =
                tokio::time::interval(std::time::Duration::from_millis(FRAME_TICK_MS));
            tick.tick().await; // first tick fires immediately; discard
            loop {
                tick.tick().await;
                let cur = frame.get();
                frame.set(cur.wrapping_add(1));
            }
        });
    }
    if let Some(v) = props.verb_override {
        verb.set(v);
    } else {
        hooks.use_future(async move {
            let mut tick =
                tokio::time::interval(std::time::Duration::from_millis(VERB_ROTATE_MS));
            tick.tick().await;
            loop {
                tick.tick().await;
                let cur = verb.get();
                verb.set(cur.wrapping_add(1));
            }
        });
    }

    let line = format_spinner_line(frame.get(), verb.get());
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: line, color: Color::Cyan)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_match_claude_code_darwin_default() {
        assert_eq!(
            SPINNER_FRAMES,
            &["·", "✢", "✳", "✶", "✻", "✽", "✽", "✻", "✶", "✳", "✢", "·"]
        );
        assert_eq!(SPINNER_FRAMES.len(), 12);
    }

    #[test]
    fn verbs_m6_match_design_subset() {
        assert_eq!(VERBS_M6, &["Crunching", "Thinking", "Generating"]);
    }

    #[test]
    fn verb_format_uses_horizontal_ellipsis() {
        assert_eq!(format!("{}{}", "Crunching", '…'), "Crunching…");
        // U+2026 HORIZONTAL ELLIPSIS, NOT three ASCII dots.
        assert_eq!('…' as u32, 0x2026);
    }

    #[test]
    fn frame_at_index_wraps() {
        assert_eq!(frame_at_index(0), "·");
        assert_eq!(frame_at_index(5), "✽");
        assert_eq!(frame_at_index(9), "✳");
        assert_eq!(frame_at_index(12), "·"); // wraps
        assert_eq!(frame_at_index(25), "✢"); // wraps twice
    }

    #[test]
    fn verb_at_index_wraps() {
        assert_eq!(verb_at_index(0), "Crunching");
        assert_eq!(verb_at_index(1), "Thinking");
        assert_eq!(verb_at_index(2), "Generating");
        assert_eq!(verb_at_index(3), "Crunching"); // wraps
    }

    #[test]
    fn format_spinner_line_includes_ellipsis() {
        assert_eq!(format_spinner_line(0, 0), "· Crunching…");
        assert_eq!(format_spinner_line(5, 1), "✽ Thinking…");
        assert_eq!(format_spinner_line(9, 2), "✳ Generating…");
    }
}
