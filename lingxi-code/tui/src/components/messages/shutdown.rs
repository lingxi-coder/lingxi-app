//! `ShutdownMessage` — teammate shutdown request/rejected notice.
//!
//! Literal lock (claude-code `ShutdownMessage.tsx`): request → `Shutdown
//! request from {from}` (warning, bold) + optional `Reason: {reason}`, round
//! warning border. rejected → `Shutdown rejected by {from}` (subtle, bold) +
//! `Reason: {reason}` + `Teammate is continuing to work. You may request
//! shutdown again later.` (dim).
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::render_iocraft::StyleColorIocraftExt;
use crate::theme::Theme;

/// Locked tail line for rejected shutdowns.
pub const REJECTED_TAIL: &str =
    "Teammate is continuing to work. You may request shutdown again later.";

/// Props for [`ShutdownMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct ShutdownProps {
    /// Originating teammate id.
    pub from: String,
    /// Optional reason.
    pub reason: Option<String>,
    /// `true` → rejected response; `false` → request.
    pub rejected: bool,
    /// (M7-15) Active palette — warning/dim colors centralized here.
    pub theme: Theme,
}

/// Pure-string renderer.
#[must_use]
pub fn render_shutdown_to_string(props: ShutdownProps) -> String {
    let mut out = if props.rejected {
        format!("Shutdown rejected by {}", props.from)
    } else {
        format!("Shutdown request from {}", props.from)
    };
    if let Some(reason) = &props.reason {
        out.push('\n');
        out.push_str(&format!("Reason: {reason}"));
    }
    if props.rejected {
        out.push('\n');
        out.push_str(REJECTED_TAIL);
    }
    out
}

/// iocraft component. Warning header for requests, subtle (dim) for rejected.
///
/// (M7-15) Colors centralized into the active [`Theme`]: requests use
/// `theme.warning` + a round warning border (claude-code), rejected use
/// `theme.dim` (subtle) with no border.
#[component]
pub fn ShutdownMessage(props: &ShutdownProps) -> impl Into<AnyElement<'static>> {
    let body = render_shutdown_to_string(props.clone());
    let theme = props.theme;
    if props.rejected {
        element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: body, color: theme.dim.to_iocraft())
            }
        }
        .into_any()
    } else {
        // Request: warning color + round warning border (M7-04 deferred → restored).
        element! {
            View(
                flex_direction: FlexDirection::Column,
                border_style: BorderStyle::Round,
                border_color: theme.warning.to_iocraft(),
            ) {
                Text(content: body, color: theme.warning.to_iocraft())
            }
        }
        .into_any()
    }
}
