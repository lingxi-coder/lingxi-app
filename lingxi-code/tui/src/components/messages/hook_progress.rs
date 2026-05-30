//! `HookProgressMessage` — running / transcript-summary hook line.
//!
//! Literal lock (claude-code `HookProgressMessage.tsx`): running →
//! `Running {event} hook…` / `Running {event} hooks…`; transcript summary
//! (Pre/PostToolUse) → `{n} {event} hook ran` / `{n} {event} hooks ran`.
//! Singular when `count == 1`. `{event}` is bold dim; the rest dim.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::Theme;

/// Props for [`HookProgressMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct HookProgressProps {
    /// Hook event name (e.g. `"PreToolUse"`).
    pub event: String,
    /// In-progress hook count for this event.
    pub count: u32,
    /// `true` → static transcript summary (`{n} … ran`); `false` → live
    /// (`Running … …`).
    pub transcript_summary: bool,
    /// (M7-15) Active palette — the dim color is centralized here.
    pub theme: Theme,
}

/// Pure-string renderer.
#[must_use]
pub fn render_hook_progress_to_string(props: HookProgressProps) -> String {
    let event = &props.event;
    if props.transcript_summary {
        let unit = if props.count == 1 { "hook" } else { "hooks" };
        format!("{n} {event} {unit} ran", n = props.count)
    } else {
        let unit = if props.count == 1 {
            "hook\u{2026}"
        } else {
            "hooks\u{2026}"
        };
        format!("Running {event} {unit}")
    }
}

/// iocraft component — all dim; `{event}` is bold in claude-code.
///
/// (M7-15) The dim color is now centralized into the active [`Theme`]. The
/// per-run bold on `{event}` is a styling nicety the string oracle ignores;
/// splitting the line into 3 colored runs here risks the line-layout
/// invariant the M7-03 `VirtualMessageList` height proxy tracks, so it is
/// consciously re-deferred — see the `TODO(M8)` below. The one-row layout +
/// dim look match claude-code's equivalent appearance.
#[component]
pub fn HookProgressMessage(props: &HookProgressProps) -> impl Into<AnyElement<'static>> {
    // TODO(M8): split `{event}` into a bold run (per-run styling). Deferred
    // from M7-15 to keep the single-row layout the height proxy assumes.
    let body = render_hook_progress_to_string(props.clone());
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: props.theme.dim)
        }
    }
}
