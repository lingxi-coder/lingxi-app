//! `ShutdownMessage` — teammate shutdown request/rejected notice.
//!
//! Literal lock (claude-code `ShutdownMessage.tsx`): request → `Shutdown
//! request from {from}` (warning, bold) + optional `Reason: {reason}`, round
//! warning border. rejected → `Shutdown rejected by {from}` (subtle, bold) +
//! `Reason: {reason}` + `Teammate is continuing to work. You may request
//! shutdown again later.` (dim).
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

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
#[component]
pub fn ShutdownMessage(props: &ShutdownProps) -> impl Into<AnyElement<'static>> {
    let body = render_shutdown_to_string(props.clone());
    // TODO(M7-15): warning/subtle round border via theme; for now color the
    // whole block (warning yellow for requests, subtle/dim for rejected).
    let color = if props.rejected {
        TuiTheme::DIM
    } else {
        Color::Yellow
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color)
        }
    }
}
